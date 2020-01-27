mod batch;
mod discord;

use std::cmp::{max, Ordering};
use std::collections::BinaryHeap;
use std::pin::Pin;
use std::sync::{Arc, RwLock};
use std::task::{Context, Poll};
use std::time::Duration;

use batch::ChunksTimeoutStreamExt;
use clap::clap_app;
use crossbeam::sync::WaitGroup;
use discord::{Client as DiscordClient, RangeParam};
use failure::Fail;
use futures::{Stream, StreamExt};
use influent::client::http::HttpClient as InfluxClient;
use influent::client::{Client, Precision};
use influent::measurement::{Measurement, Value};
use influent::serializer::line::LineSerializer;

type AnyError = Box<dyn std::error::Error>;
type AnyResult<T> = Result<T, AnyError>;

#[derive(Debug, Fail)]
enum Error {
    #[fail(display = "Cannot parse guild \"{}\".", _0)]
    InvalidGuild(String),
}

#[tokio::main]
async fn main() -> AnyResult<()> {
    let matches = clap_app!(("discord-influx") =>
        (version: "0.1")
        (author: "Richard Patel <me@terorie.dev>")
        (about: "Discord to Influx exporter")
        (about: "Live streams message stats to database")
        (@arg host: -h --host +takes_value ... default_value("http://localhost:8086") "Influx hosts")
        (@arg start: -s --start +takes_value default_value("latest") "Start ID")
        (@arg database: -d --database +takes_value default_value("discord") "Influx database")
        (@arg all: -a --all "Try to dump all channels, including hidden ones")
        (@arg guilds: ... +required "Database guild handles (\"guild1/1337\", \"guild2/42069\"...)")
    ).get_matches();

    // Build the Discord client
    let discord_token = match std::env::var("DISCORD_TOKEN") {
        Ok(t) => t,
        Err(std::env::VarError::NotPresent) => {
            eprintln!("Variable $DISCORD_TOKEN is required!");
            std::process::exit(1);
        }
        Err(e) => panic!(e),
    };
    let discord = DiscordClient::new(&discord_token);

    // Build the Influx client
    let user_env = std::env::var("INFLUX_USERNAME")
        .ok()
        .unwrap_or_else(|| "".to_owned());
    let pass_env = std::env::var("INFLUX_PASSWORD")
        .ok()
        .unwrap_or_else(|| "".to_owned());
    let credentials = influent::client::Credentials {
        username: Box::leak(user_env.into_boxed_str()),
        password: Box::leak(pass_env.into_boxed_str()),
        database: "discord",
    };
    let serializer = LineSerializer::new();
    let hurl = influent::hurl::reqwest::ReqwestHurl::new();
    let mut influx = InfluxClient::new(credentials, Box::new(serializer), Box::new(hurl));
    let host_matches = matches.values_of_lossy("host").unwrap();
    for host in host_matches {
        let host_leak = Box::leak(host.into_boxed_str());
        influx.add_host(host_leak);
    }
    let influx = Arc::new(influx);

    // --start flag
    let mut start_override = Some(matches.value_of("start").unwrap());
    if start_override == Some("latest") {
        start_override = None;
    }

    // --all flag
    let dump_all = matches.is_present("all");

    // Process guilds
    let guild_matches = matches.values_of_lossy("guilds").unwrap();
    let wg = WaitGroup::new();
    for guild_match in guild_matches {
        let (guild_handle, guild_id) = match parse_guild_input(&guild_match) {
            None => return Err(Error::InvalidGuild(guild_match).compat().into()),
            Some(v) => v,
        };
        eprintln!("Processing {}", guild_handle);
        let discord = discord.clone();
        let channels = discord
            .guild_get_channels(&guild_id)
            .await?
            .into_iter()
            // Only fetch text channels
            .filter(|c| c.channel_type == 0)
            // If no --all flag, omit "hidden" channels.
            // I interpret hidden as "has no deny overwrite for READ_MESSAGE_HISTORY on @everyone".
            .filter(|c| {
                dump_all
                    || c.permission_overwrites
                        .as_ref()
                        // Search for that overwrite
                        .and_then(|perms| {
                            perms.iter().find(|p| {
                                p.id == guild_id
                                    && p.deny
                                        & (discord::READ_MESSAGE_HISTORY | discord::VIEW_CHANNEL)
                                        != 0
                            })
                        })
                        // If no such overwrite was found, it passes the filter
                        .is_none()
            });
        for mut channel in channels {
            eprintln!("Polling {} #{}", guild_handle, channel.name);

            if let Some(start) = start_override {
                channel.last_message_id = Some(start.to_string());
            }
            let discord = discord.clone();
            let influx = Arc::clone(&influx);
            let guild_handle = guild_handle.clone();
            let wg = wg.clone();
            tokio::spawn(async {
                stream_to_influx(discord, influx, guild_handle, channel).await;
                drop(wg);
            });
        }
    }
    wg.wait();
    Ok(())
}

fn parse_guild_input(input: &str) -> Option<(String, String)> {
    let split: Vec<&str> = input.splitn(2, '/').collect();
    if split.len() <= 1 {
        None
    } else {
        Some((split[0].to_owned(), split[1].to_owned()))
    }
}

async fn stream_to_influx(
    discord: DiscordClient,
    influx: Arc<InfluxClient<'_>>,
    guild_handle: String,
    chan: discord::Channel,
) {
    let start = RangeParam::After(chan.last_message_id.unwrap_or_else(|| "0".to_owned()));
    let chan_name = chan.name.clone();
    let messages = stream_messages(&discord, &chan.id, start, Duration::from_secs(3))
        .map(move |message| Message {
            time: message.timestamp.timestamp() / 60,
            location: Location {
                guild: guild_handle.clone(),
                channel: chan_name.clone(),
            },
        })
        .fuse();
    // Buffer 3s worth of messages.
    // Since chat history in a single channel is guaranteed
    // to be ordered anyways, buffering should be obsolete.
    MessageAggregate::new(Box::pin(messages), 3)
        .map(|aggs| futures::stream::iter(aggs.into_iter()))
        .flatten()
        // Convert to InfluxDB measurements
        .map(|agg| {
            let mut m = Measurement::new("messages");
            m.add_tag("guild", agg.location.guild);
            m.add_tag("channel", agg.location.channel);
            m.add_field("count", Value::Integer(agg.count as i64));
            m.set_timestamp(agg.time);
            m
        })
        // Commit batches of max 128 messages every 500ms
        .chunks_timeout(128, Duration::from_millis(500))
        // Write batches to InfluxDB
        .then(|m| async {
            let m = std::sync::Arc::new(m);
            let res = influx.write_many(&m, Some(Precision::Minutes)).await;
            (res, m.len())
        })
        // Handle errors
        .for_each(|(res, count)| async move {
            if let Err(e) = res {
                eprintln!("Influx error: {:?}", e);
            } else {
                eprintln!("Flushed {} aggregations", count);
            }
        })
        .await;
}

// Streams all messages in the channel in ascending order, beginning with start.
// Stops if no new messages are found.
fn stream_messages_to_end<'a>(
    discord: &'a DiscordClient,
    channel_id: &'a str,
    start: RangeParam<String>,
) -> impl Stream<Item = discord::Message> + 'a {
    // A stream of message lists
    let message_vecs = futures::stream::unfold(start, move |position| async move {
        // Get the next message list
        match discord
            .channel_get_messages(channel_id, position, 100)
            .await
        {
            Ok(mut messages) => {
                messages.reverse();
                if messages.is_empty() {
                    // It's empty, end is reached
                    None
                } else {
                    // Push message list, and continue after last message in list
                    let next_pos =
                        RangeParam::After(messages.last().expect("No last message").id.clone());
                    Some((messages, next_pos))
                }
            }
            Err(e) => {
                eprintln!("Message stream aborted: {}", e);
                None
            }
        }
    });
    message_vecs
        // A stream of streams of messages
        .map(|messages| futures::stream::iter(messages.into_iter()))
        // A stream of messages
        .flatten()
}

// Indefinetely streams all messages in the channel in ascending order, begging with start.
fn stream_messages<'a>(
    discord: &'a DiscordClient,
    channel_id: &'a str,
    start: RangeParam<String>,
    poll_i: Duration,
) -> impl Stream<Item = discord::Message> + 'a {
    let start = Arc::new(RwLock::new(start));
    let stream_of_streams =
        futures::stream::unfold((start, true), move |(start, first)| async move {
            // Cool down before getting next stream
            if !first {
                tokio::time::delay_for(poll_i).await;
            }
            // Stream until end is reached
            let start_copy = Arc::clone(&start);
            let part_start = start.read().unwrap().clone();
            let stream = stream_messages_to_end(discord, channel_id, part_start)
                // Update the position
                .inspect(move |m| *(start.write().unwrap()) = RangeParam::After(m.id.clone()));
            Some((stream, (start_copy, false)))
        });
    stream_of_streams.flatten()
}

#[derive(Clone, Debug, Eq, PartialEq, Hash, Ord, PartialOrd)]
pub struct Location {
    pub guild: String,
    pub channel: String,
}

#[derive(Clone, Eq, PartialEq)]
pub struct Message {
    pub time: i64,
    pub location: Location,
}

impl Ord for Message {
    fn cmp(&self, other: &Self) -> Ordering {
        other
            .time
            .cmp(&self.time)
            .then_with(|| self.location.cmp(&other.location))
    }
}

impl PartialOrd for Message {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

struct Aggregator {
    messages: BinaryHeap<Message>,
    min_time: i64,
    max_time: Option<i64>,
    threshold: i64,
}

#[derive(Debug)]
pub struct Aggregation {
    pub location: Location,
    pub time: i64,
    pub count: u64,
}

impl Aggregator {
    pub fn new(threshold: i64) -> Self {
        Self {
            messages: BinaryHeap::new(),
            min_time: 0,
            max_time: None,
            threshold,
        }
    }

    pub fn push(&mut self, message: Message) -> Option<Vec<Aggregation>> {
        if message.time < self.min_time {
            // Ignore message that was probably already committed.
            return None;
        }

        self.max_time = Some(max(self.max_time.unwrap_or(0), message.time));
        self.messages.push(message);

        if self.max_time.expect("No max_time") - self.min_time <= self.threshold {
            return None;
        }

        // Threshold exceeded, start outputting aggregations.
        let mut aggs = Vec::new();
        loop {
            if let Some(agg) = self.next_aggregation() {
                aggs.push(agg);
            } else {
                break;
            }
            if self.max_time.unwrap_or(0) - self.min_time > self.threshold {
                break;
            }
        }

        Some(aggs)
    }

    // Remove the current message group and count items.
    // A message group are messages with the same
    // (time, guild_id, channel_id).
    fn next_aggregation(&mut self) -> Option<Aggregation> {
        if self.messages.is_empty() {
            return None;
        }
        // Process a group
        let peek_msg = self.messages.peek().expect("No message to peek");
        let location = peek_msg.location.clone();
        let time = peek_msg.time;
        let mut count = 0u64;
        while let Some(msg) = self.messages.peek() {
            self.min_time = msg.time;
            if msg.time != time || msg.location != location {
                break;
            }
            let msg = self.messages.pop().expect("No message to pop");
            count += 1;
            debug_assert!(
                msg.time >= self.min_time,
                "New message in front of old message"
            );
        }
        debug_assert!(self
            .messages
            .peek()
            .map(|msg| msg.time >= self.min_time)
            .unwrap_or(true));
        if self.messages.is_empty() {
            self.max_time = None;
        }
        Some(Aggregation {
            location,
            time,
            count,
        })
    }

    pub fn flush(&mut self) -> Option<Vec<Aggregation>> {
        if self.messages.is_empty() {
            return None;
        }
        let mut aggs = Vec::new();
        while let Some(agg) = self.next_aggregation() {
            aggs.push(agg);
        }
        if aggs.is_empty() {
            None
        } else {
            Some(aggs)
        }
    }
}

pub struct MessageAggregate<'a> {
    stream: Pin<Box<dyn Stream<Item = Message> + Send + 'a>>,
    aggregator: Aggregator,
}

impl<'a> MessageAggregate<'a> {
    fn new(stream: Pin<Box<dyn Stream<Item = Message> + Send + 'a>>, window: i64) -> Self {
        Self {
            aggregator: Aggregator::new(window),
            stream,
        }
    }
}

impl<'a> Stream for MessageAggregate<'a> {
    type Item = Vec<Aggregation>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context) -> Poll<Option<Self::Item>> {
        loop {
            match Stream::poll_next(self.stream.as_mut(), cx) {
                // Waiting for next message
                Poll::Pending => return Poll::Pending,
                // Another message is ready
                Poll::Ready(Some(message)) => match self.aggregator.push(message) {
                    None => continue,
                    Some(aggregations) => return Poll::Ready(Some(aggregations)),
                },
                // Source iterator has ended
                Poll::Ready(None) => return Poll::Ready(self.aggregator.flush()),
            }
        }
    }
}

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use chrono::{DateTime, Utc};
use reqwest::header;
use serde::{Serialize, Serializer};
use serde_derive::Deserialize;
use tokio::time::Instant;

#[derive(Clone)]
pub struct Client {
    pub client: reqwest::Client,
    pub max_retries: i32,
    pub default_delay: Duration,
}

impl Client {
    pub fn new(token: &str) -> Self {
        // Build default headers
        let mut headers = header::HeaderMap::new();
        headers.insert(
            header::AUTHORIZATION,
            header::HeaderValue::from_str(token).expect("Illegal token"),
        );
        // Build HTTP client
        let client = reqwest::Client::builder()
            .default_headers(headers)
            .build()
            .expect("Failed to build HTTP client");
        Self {
            client,
            max_retries: 50,
            default_delay: Duration::from_secs(1),
        }
    }

    pub async fn guild_get_channels(&self, guild_id: &str) -> reqwest::Result<Vec<Channel>> {
        let url = format!(
            "https://discordapp.com/api/v6/guilds/{}/channels",
            url_escape(guild_id)
        );
        self.client
            .get(&url)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await
    }

    pub async fn channel_get_messages(
        &self,
        channel_id: &str,
        param: RangeParam<String>,
        limit: i32,
    ) -> reqwest::Result<Vec<Message>> {
        let url = format!(
            "https://discordapp.com/api/v6/channels/{}/messages",
            url_escape(channel_id)
        );
        self.get_retry(|| {
            self.client
                .get(&url)
                .query(&[("limit", limit)])
                .query(&[&param])
                .build()
                .unwrap()
        })
        .await?
        .json()
        .await
    }

    pub(crate) async fn get_retry<T>(&self, make_request: T) -> reqwest::Result<reqwest::Response>
    where
        T: Fn() -> reqwest::Request,
    {
        let mut delay_override: Option<Instant> = None;
        let mut last_err: Option<reqwest::Error> = None;
        for i in 0..self.max_retries {
            if i != 0 {
                let instant = delay_override.unwrap_or(Instant::now() + self.default_delay);
                tokio::time::delay_until(instant).await;
            }
            let res = self
                .client
                .execute(make_request())
                .await
                .map(|resp| {
                    if resp.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
                        delay_override = resp
                            .headers()
                            .get("X-RateLimit-Reset")
                            .and_then(|h| h.to_str().ok())
                            .and_then(|s| s.parse::<u64>().ok())
                            .map(Duration::from_secs)
                            .map(|d| (UNIX_EPOCH + d))
                            .and_then(|t| t.duration_since(SystemTime::now()).ok())
                            .map(|d| {
                                eprintln!("Rate limited, retrying in {} ms", d.as_millis());
                                Instant::now() + d
                            });
                    }
                    resp
                })
                .and_then(|resp| resp.error_for_status());
            match res {
                Err(e) => {
                    last_err = Some(e);
                    continue;
                }
                Ok(v) => return Ok(v),
            };
        }
        Err(last_err.unwrap())
    }
}

#[allow(dead_code)]
#[derive(Clone, Debug)]
pub enum RangeParam<T> {
    Around(T),
    Before(T),
    After(T),
}

/*impl<T> RangeParam<T> {
    pub fn map<U, F: FnOnce(T) -> U>(self, f: F) -> RangeParam<U> {
        match self {
            RangeParam::Around(x) => RangeParam::Around(f(x)),
            RangeParam::Before(x) => RangeParam::Before(f(x)),
            RangeParam::After(x) => RangeParam::After(f(x)),
        }
    }
}*/

impl<T: Serialize> Serialize for RangeParam<T> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            RangeParam::Around(v) => ("around", v),
            RangeParam::Before(v) => ("before", v),
            RangeParam::After(v) => ("after", v),
        }
        .serialize(serializer)
    }
}

#[derive(Deserialize)]
pub struct Channel {
    pub id: String,
    #[serde(rename = "type")]
    pub channel_type: i32,
    pub name: String,
    pub last_message_id: Option<String>,
    pub permission_overwrites: Option<Vec<Overwrite>>,
}

#[derive(Deserialize)]
pub struct Message {
    pub id: String,
    pub channel_id: String,
    pub timestamp: DateTime<Utc>,
    pub content: String,
    pub author: Author,
}

#[derive(Deserialize)]
pub struct Author {
    pub id: String,
    pub username: String,
}

#[derive(Deserialize)]
pub struct Overwrite {
    pub id: String,
    #[serde(rename = "type")]
    pub overwrite_type: OverwriteType,
    pub allow: u64,
    pub deny: u64,
}

#[derive(Deserialize)]
pub enum OverwriteType {
    #[serde(rename = "role")]
    Role,
    #[serde(rename = "member")]
    Member,
}

pub const VIEW_CHANNEL: u64 = 0x0000_0400;
pub const READ_MESSAGE_HISTORY: u64 = 0x0001_0000;

fn url_escape(segment: &'_ str) -> percent_encoding::PercentEncode<'_> {
    percent_encoding::utf8_percent_encode(segment, percent_encoding::NON_ALPHANUMERIC)
}

package main

import (
	"os"
	"os/signal"

	"github.com/bwmarrin/discordgo"
	influxdb2 "github.com/influxdata/influxdb-client-go/v2"
	"github.com/influxdata/influxdb-client-go/v2/api"
	"github.com/influxdata/influxdb-client-go/v2/api/write"
	"github.com/spf13/cobra"
	"github.com/spf13/pflag"
	"go.uber.org/zap"
)

var live = cobra.Command{
	Use:   "live",
	Short: "Continually export live stats",
	Long:  "Exports live event statistics from the channels the Discord Bot is in.",
	Args:  cobra.NoArgs,
	PreRun: func(_ *cobra.Command, _ []string) {
		initDiscord()
	},
	Run: runLive,
}

func init() {
	addInfluxFlags(live.Flags())
}

func runLive(c *cobra.Command, _ []string) {
	influx := newInfluxContextFromFlags(c.Flags())
	defer influx.close()
	defer func() { _ = discord.Close() }()
	log.Info("Starting Discord-InfluxDB live exporter")
	defer log.Info("Stopping")
	discord.AddHandler(func(s *discordgo.Session, m *discordgo.MessageCreate) {
		log := log.With(
			zap.String("guild_id", m.GuildID),
			zap.String("channel_id", m.ChannelID),
			zap.String("message_id", m.Message.ID))
		timestamp := messageTimestamp(m.Message.ID)
		influx.writeAPI.WritePoint(write.NewPointWithMeasurement(metricMessages).
			SetTime(timestamp).
			AddTag(labelGuild, m.GuildID).
			AddTag(labelChannel, m.ChannelID).
			AddField(fieldCount, 1))
		influx.writeAPI.WritePoint(write.NewPointWithMeasurement(metricUserMessages).
			SetTime(timestamp).
			AddTag(labelGuild, m.GuildID).
			AddTag(labelUser, m.Author.String()).
			AddField(fieldCount, 1))
		log.Debug("MessageCreate")
	})
	discord.AddHandler(func(s *discordgo.Session, m *discordgo.MessageReactionAdd) {
		log := log.With(
			zap.String("guild_id", m.GuildID),
			zap.String("channel_id", m.ChannelID),
			zap.String("message_id", m.MessageID),
			zap.String("emoji", m.Emoji.Name))
		influx.writeAPI.WritePoint(write.NewPointWithMeasurement(metricReactions).
			SetTime(messageTimestamp(m.MessageID)).
			AddTag(labelGuild, m.GuildID).
			AddTag(labelEmoji, m.Emoji.Name).
			AddField(fieldCount, 1))
		log.Debug("MessageReactionAdd")
	})
	discord.AddHandler(func(s *discordgo.Session, m *discordgo.MessageReactionRemove) {
		log := log.With(
			zap.String("guild_id", m.GuildID),
			zap.String("channel_id", m.ChannelID),
			zap.String("message_id", m.MessageID),
			zap.String("emoji", m.Emoji.Name))
		timestamp := messageTimestamp(m.MessageID)
		influx.writeAPI.WritePoint(write.NewPointWithMeasurement(metricReactions).
			SetTime(timestamp).
			AddTag(labelGuild, m.GuildID).
			AddTag(labelEmoji, m.Emoji.Name).
			AddField(fieldCount, -1))
		log.Debug("MessageReactionRemove")
	})
	discord.Identify.Intents = discordgo.IntentsGuildMessages | discordgo.IntentsGuildMessageReactions
	if err := discord.Open(); err != nil {
		log.Fatal("Failed to connect to Discord", zap.Error(err))
	}
	interrupt := make(chan os.Signal, 1)
	signal.Notify(interrupt, os.Interrupt)
	<-interrupt
}

type influxContext struct {
	client   influxdb2.Client
	writeAPI api.WriteAPI
}

func newInfluxContextFromFlags(f *pflag.FlagSet) *influxContext {
	influxURL, err := f.GetString(flagInfluxURL)
	if err != nil {
		panic(err.Error())
	}
	if influxURL == "" {
		log.Fatal("Missing InfluxDB URL")
	}
	apiToken := readInfluxToken()
	influx := influxdb2.NewClient(influxURL, apiToken)
	influxOrg, err := f.GetString(flagInfluxOrg)
	if err != nil {
		panic(err.Error())
	}
	influxBucket, err := f.GetString(flagInfluxBucket)
	if err != nil {
		panic(err.Error())
	}
	writeAPI := influx.WriteAPI(influxOrg, influxBucket)
	return &influxContext{
		client:   influx,
		writeAPI: writeAPI,
	}
}

func (i *influxContext) close() {
	i.writeAPI.Flush()
	i.client.Close()
}

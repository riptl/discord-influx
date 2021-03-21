package main

import (
	"fmt"
	"io/ioutil"
	"os"
	"os/signal"
	"strings"
	"time"

	"github.com/bwmarrin/discordgo"
	influxdb2 "github.com/influxdata/influxdb-client-go/v2"
	"github.com/influxdata/influxdb-client-go/v2/api/write"
	"github.com/spf13/cobra"
	"github.com/spf13/pflag"
	"go.uber.org/zap"
	"go.uber.org/zap/zapcore"
)

func main() {
	if err := root.Execute(); err != nil {
		fmt.Fprintln(os.Stderr, err.Error())
	}
}

var root = cobra.Command{
	Use:   "discord-influx",
	Short: "Discord metrics exporter",
	PersistentPreRun: func(c *cobra.Command, _ []string) {
		pflags := c.Flags()
		debug, err := pflags.GetBool(flagDebug)
		if err != nil {
			panic(err.Error())
		}
		if debug {
			logLevel.SetLevel(zapcore.DebugLevel)
		} else {
			logLevel.SetLevel(zapcore.InfoLevel)
		}
	},
}

func init() {
	pflags := root.PersistentFlags()
	pflags.Bool(flagDebug, false, "Enable debug log")
	root.AddCommand(&live)
}

var live = cobra.Command{
	Use:   "live",
	Short: "Live export stats",
	Long:  "Exports live event statistics from the channels the Discord Bot is in.",
	Args:  cobra.NoArgs,
	PreRun: func(_ *cobra.Command, _ []string) {
		initDiscord()
	},
	Run: func(c *cobra.Command, args []string) {
		influxURL, err := c.Flags().GetString(flagInfluxURL)
		if err != nil {
			panic(err.Error())
		}
		if influxURL == "" {
			log.Fatal("Missing InfluxDB URL")
		}
		apiToken := readInfluxToken()
		influx := influxdb2.NewClient(influxURL, apiToken)
		defer influx.Close()
		influxOrg, err := c.Flags().GetString(flagInfluxOrg)
		if err != nil {
			panic(err.Error())
		}
		influxBucket, err := c.Flags().GetString(flagInfluxBucket)
		if err != nil {
			panic(err.Error())
		}
		writeAPI := influx.WriteAPI(influxOrg, influxBucket)
		defer writeAPI.Flush()
		defer discord.Close()
		log.Info("Starting Discord-InfluxDB live exporter")
		defer log.Info("Stopping")
		discord.AddHandler(func(s *discordgo.Session, m *discordgo.MessageCreate) {
			log := log.With(
				zap.String("guild_id", m.GuildID),
				zap.String("channel_id", m.ChannelID),
				zap.String("message_id", m.Message.ID))
			t, err := m.Timestamp.Parse()
			if err != nil {
				log.Warn("Invalid timestamp on message, ignoring", zap.Error(err))
				return
			}
			writeAPI.WritePoint(write.NewPointWithMeasurement("discord_messages").
				SetTime(t).
				AddTag("guild", m.GuildID).
				AddTag("channel", m.ChannelID).
				AddField("count", 1))
			log.Debug("MessageCreate")
		})
		discord.AddHandler(func(s *discordgo.Session, m *discordgo.MessageReactionAdd) {
			log := log.With(
				zap.String("guild_id", m.GuildID),
				zap.String("channel_id", m.ChannelID),
				zap.String("message_id", m.MessageID),
				zap.String("emoji", m.Emoji.Name))
			writeAPI.WritePoint(write.NewPointWithMeasurement("discord_message_reactions").
				AddTag("guild", m.GuildID).
				AddTag("emoji", m.Emoji.Name).
				AddField("count", 1))
			log.Debug("MessageReactionAdd")
		})
		discord.AddHandler(func(s *discordgo.Session, m *discordgo.MessageReactionRemove) {
			log := log.With(
				zap.String("guild_id", m.GuildID),
				zap.String("channel_id", m.ChannelID),
				zap.String("message_id", m.MessageID),
				zap.String("emoji", m.Emoji.Name))
			writeAPI.WritePoint(write.NewPointWithMeasurement("discord_message_reactions").
				AddTag("guild", m.GuildID).
				AddTag("emoji", m.Emoji.Name).
				AddField("count", -1))
			log.Debug("MessageReactionRemove")
		})
		discord.Identify.Intents = discordgo.IntentsGuildMessages | discordgo.IntentsGuildMessageReactions
		if err := discord.Open(); err != nil {
			log.Fatal("Failed to connect to Discord", zap.Error(err))
		}
		interrupt := make(chan os.Signal, 1)
		signal.Notify(interrupt, os.Interrupt, os.Kill)
		<-interrupt
	},
}

func init() {
	addInfluxFlags(live.Flags())
}

const (
	flagDebug        = "debug"
	flagInfluxURL    = "influxdb-url"
	flagInfluxOrg    = "influxdb-org"
	flagInfluxBucket = "influxdb-bucket"
)

func addInfluxFlags(flags *pflag.FlagSet) {
	flags.String(flagInfluxURL, "", "InfluxDB server URL")
	flags.String(flagInfluxOrg, "", "InfluxDB Organization")
	flags.String(flagInfluxBucket, "", "InfluxDB bucket")
}

var log *zap.Logger
var logLevel = zap.NewAtomicLevelAt(zap.DebugLevel)

func init() {
	config := zap.Config{
		Level:       logLevel,
		Development: true,
		Encoding:    "console",
		EncoderConfig: zapcore.EncoderConfig{
			LevelKey:    "L",
			MessageKey:  "M",
			LineEnding:  zapcore.DefaultLineEnding,
			EncodeLevel: zapcore.CapitalColorLevelEncoder,
			EncodeTime: func(t time.Time, enc zapcore.PrimitiveArrayEncoder) {
				enc.AppendString(t.Format("2006-01-02T15:04:05Z0700"))
			},
			EncodeDuration: zapcore.StringDurationEncoder,
			EncodeCaller:   zapcore.ShortCallerEncoder,
		},
		OutputPaths:       []string{"stderr"},
		ErrorOutputPaths:  []string{"stderr"},
		DisableCaller:     true,
		DisableStacktrace: true,
	}
	var err error
	log, err = config.Build()
	if err != nil {
		panic(err.Error())
	}
}

var discord *discordgo.Session

func readDiscordToken() (discordToken string) {
	discordTokenPath := os.Getenv("DISCORD_TOKEN_FILE")
	if discordTokenPath != "" {
		discordTokenBuf, err := ioutil.ReadFile(discordTokenPath)
		if err != nil {
			panic(err.Error())
		}
		discordToken = strings.TrimSpace(string(discordTokenBuf))
	} else {
		discordToken = strings.TrimSpace(os.Getenv("DISCORD_TOKEN"))
	}
	if discordToken == "" {
		log.Fatal("No Discord token found")
	}
	return
}

func initDiscord() {
	apiToken := readDiscordToken()
	var err error
	discord, err = discordgo.New(apiToken)
	if err != nil {
		log.Fatal("Error creating Discord token", zap.Error(err))
	}
}

func readInfluxToken() (influxToken string) {
	influxTokenPath := os.Getenv("INFLUXDB_TOKEN_FILE")
	if influxTokenPath != "" {
		influxTokenBuf, err := ioutil.ReadFile(influxTokenPath)
		if err != nil {
			panic(err.Error())
		}
		influxToken = strings.TrimSpace(string(influxTokenBuf))
	} else {
		influxToken = strings.TrimSpace(os.Getenv("INFLUXDB_TOKEN"))
	}
	if influxToken == "" {
		log.Fatal("No influx token found")
	}
	return
}

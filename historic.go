package main

import (
	"strconv"
	"strings"

	"github.com/bwmarrin/discordgo"
	"github.com/influxdata/influxdb-client-go/v2/api/write"
	"github.com/spf13/cobra"
	"go.uber.org/zap"
)

var historic = cobra.Command{
	Use:   "historic <guild_id/channel_id> ...",
	Short: "Export historic stats",
	Long:  "One-off job to export historic statistics of specific channels.",
	Args:  cobra.MinimumNArgs(1),
	PreRun: func(_ *cobra.Command, _ []string) {
		initDiscord()
	},
	Run: runHistoric,
}

func init() {
	flags := historic.Flags()
	addInfluxFlags(flags)
	flags.String("start", "0", "Export messages after this ID or RFC 3339 timestamp")
	flags.String("stop", "2199-12-31T23:59:59Z", "Export messages before this ID or RFC 3339 timestamp")
}

func runHistoric(c *cobra.Command, args []string) {
	// Parse bounds.
	flags := c.Flags()
	startFlag, err := flags.GetString("start")
	if err != nil {
		panic(err.Error())
	}
	stopFlag, err := flags.GetString("stop")
	if err != nil {
		panic(err.Error())
	}
	b := bounds{
		start: parseTimeOrID(startFlag),
		stop:  parseTimeOrID(stopFlag),
	}
	influx := newInfluxContextFromFlags(c.Flags())
	defer influx.close()
	defer func() { _ = discord.Close() }()
	// Parse guild/channel targets.
	targets := make(map[channelTarget]struct{})
	for _, arg := range args {
		parts := strings.SplitN(arg, "/", 2)
		if len(parts) == 1 {
			// All channels in guild.
			guildID := parts[0]
			channels, err := discord.GuildChannels(guildID)
			if err != nil {
				log.Fatal("Failed to get channels for guild", zap.Error(err),
					zap.String("guild", guildID))
			}
			for _, channel := range channels {
				if channel.Type != discordgo.ChannelTypeGuildText {
					continue
				}
				target := channelTarget{
					GuildID:   channel.GuildID,
					ChannelID: channel.ID,
				}
				targets[target] = struct{}{}
			}
		} else if len(parts) == 2 {
			// A specific channel in a guild.
			target := channelTarget{GuildID: parts[1], ChannelID: parts[2]}
			targets[target] = struct{}{}
		}
	}
	// Scrape messages for each target.
	for target := range targets {
		exportHistoric(influx, target, b)
	}
	log.Info("Done")
}

type channelTarget struct {
	GuildID   string
	ChannelID string
}

type bounds struct {
	start, stop int64
}

func exportHistoric(influx *influxContext, target channelTarget, b bounds) {
	log := log.With(zap.String("guild", target.GuildID), zap.String("channel", target.ChannelID))
	beforeID := strconv.FormatInt(b.stop, 10)
	log.Info("Starting export", zap.String("before_id", beforeID))
mainLoop:
	for i := 0; true; i++ {
		log.Debug("Processing page", zap.Int("page", i), zap.String("before_id", beforeID))
		const limit = 100
		messages, err := discord.ChannelMessages(target.ChannelID, limit, beforeID, "", "")
		if err != nil {
			log.Error("Export failed", zap.Error(err))
		}
		if len(messages) == 0 {
			break
		}
		for _, message := range messages {
			msgID := parseMessageID(message.ID)
			if msgID < b.start {
				break mainLoop
			}
			log.Debug("Processing message",
				zap.Int64("message_id", msgID),
				zap.Time("msg_time", messageTimestamp(message.ID)))
			exportHistoricMessage(influx, message, target.GuildID)
			beforeID = message.ID
		}
	}
}

func exportHistoricMessage(influx *influxContext, msg *discordgo.Message, guildID string) {
	timestamp := messageTimestamp(msg.ID)
	influx.writeAPI.WritePoint(write.NewPointWithMeasurement(metricMessages).
		SetTime(timestamp).
		AddTag(labelGuild, guildID).
		AddTag(labelChannel, msg.ChannelID).
		AddField(fieldCount, 1))
	for _, reaction := range msg.Reactions {
		if reaction.Emoji == nil {
			continue
		}
		influx.writeAPI.WritePoint(write.NewPointWithMeasurement(metricReactions).
			SetTime(timestamp).
			AddTag(labelGuild, guildID).
			AddTag(labelEmoji, reaction.Emoji.Name).
			AddField(fieldCount, 1))
	}
}

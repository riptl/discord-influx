package main

import (
	"strconv"
	"time"
)

func parseMessageID(msgIDStr string) int64 {
	msgID, err := strconv.ParseInt(msgIDStr, 10, 64)
	if err != nil {
		// We don't ever expect Discord to return invalid timestamps in Snowflakes.
		// If they do, it's better to just abort the program.
		panic("invalid message ID: " + msgIDStr)
	}
	return msgID
}

// messageTimestamp converts a Discord message ID to a timestamp.
//
// This uses a deterministic algorithm to ensure different message IDs result in slightly different timestamps.
// This is done so separate messages sent at the same time result in separate InfluxDB data points.
// A value derived from the Snowflake shard and sequence numbers skews the timestamp from 0-524287 µs to achieve this.
//
// Providing an invalid ID aborts the program for safety.
func messageTimestamp(msgIDStr string) time.Time {
	msgID := parseMessageID(msgIDStr)
	epoch := (msgID >> 22) + 1_420_070_400_000 // Take epoch starting at first second in 2015.
	nsec := epoch * 1e6
	shard := (msgID >> 15) & 0x007f // Take 7 most significant bits of the 10-bit shard ID.
	sequence := msgID & 0x0fff      // Take 12-bit sequence ID.
	nsec += shard<<12 | sequence    // Pack into millisecond fraction. (19 bits)
	return time.Unix(0, nsec)
}

func parseTimeOrID(arg string) int64 {
	num, err := strconv.ParseInt(arg, 10, 64)
	if err == nil {
		return num
	}
	t, err := time.Parse(time.RFC3339, arg)
	if err == nil {
		millis := t.UnixNano() / 1e6
		epoch := millis - 1_420_070_400_000
		return epoch << 22
	}
	log.Fatal("Invalid argument: " + arg)
	panic("unreachable")
}

package main

import (
	"testing"
	"time"
)

func TestMessageTimestamp(t *testing.T) {
	ts := messageTimestamp("828774868774420520")
	// Snowflake:
	// epoch 0b10111000000001100110010110100010010000
	// shard 0b100100000
	// seq   0b000000101000
	unix := ts.Unix()
	const expUnix = int64(1617665726)
	if unix != expUnix {
		t.Fatalf("expected unix %d got %d", expUnix, unix)
	}
	const expMillis = int64(608)
	millis := ts.UnixNano() % 1e9 / 1e6
	if millis != expMillis {
		t.Fatalf("expected millis %d got %d", expMillis, millis)
	}
	const expNano = int64(0b0100100_000000101000)
	nano := ts.UnixNano() % 1e6
	if nano != expNano {
		t.Fatalf("expected nano %x got %x", expNano, nano)
	}
	t.Log(ts.String())
}

func TestParseTimeOrID(t *testing.T) {
	t.Run("ID", func(t *testing.T) {
		const exp = int64(828774868774420520)
		num := parseTimeOrID("828774868774420520")
		if num != exp {
			t.Fatalf("expected %d got %d", exp, num)
		}
	})
	t.Run("Time", func(t *testing.T) {
		ts := time.Date(2021, 4, 6, 3, 1, 1, 1, time.UTC).Format(time.RFC3339)
		num := parseTimeOrID(ts)
		const exp = int64(828826602962944000)
		if num != exp {
			t.Fatalf("expected %d got %d", exp, num)
		}
	})
}

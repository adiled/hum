package thrum

import (
	"strings"
	"testing"
)

// Byte-for-byte parity with hum_identity's encoder. These vectors come
// straight from ids.rs ts_parity_vectors — if hum_identity's alphabet or
// shift layout ever changes, this test names the drift.
func TestHumIdParityVectors(t *testing.T) {
	var zero [32]byte
	if got := HumIdEncode(zero); got != strings.Repeat("0", 52) {
		t.Fatalf("zeros: got %q", got)
	}

	// 1_700_000_000_000 ms in the top 6 bytes, 208 zero bits after.
	ts := uint64(1700000000000)
	var tsOnly [32]byte
	for i := 0; i < 6; i++ {
		tsOnly[5-i] = byte(ts >> (8 * uint(i)))
	}
	want := "065WZSB8" + strings.Repeat("0", 44)
	if got := HumIdEncode(tsOnly); got != want {
		t.Fatalf("ts-only: got %q want %q", got, want)
	}

	var ones [32]byte
	for i := range ones {
		ones[i] = 0xff
	}
	want = strings.Repeat("Z", 51) + "G"
	if got := HumIdEncode(ones); got != want {
		t.Fatalf("ones: got %q want %q", got, want)
	}
}

func TestRidIsValidCrockford52(t *testing.T) {
	for i := 0; i < 100; i++ {
		id := Rid()
		if !IsValidRid(id) {
			t.Fatalf("Rid()=%q invalid", id)
		}
		if len(id) != 52 {
			t.Fatalf("Rid()=%q len %d", id, len(id))
		}
	}
	if IsValidRid("") || IsValidRid(strings.Repeat("I", 52)) {
		t.Fatal("IsValidRid accepted garbage")
	}
}

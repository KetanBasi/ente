package forkworker

import (
	"encoding/base64"
	"strings"
	"testing"

	"github.com/ente/museum/ente/forkworker"
)

func TestValidateKeys(t *testing.T) {
	good := base64.StdEncoding.EncodeToString(make([]byte, 32))
	other := base64.StdEncoding.EncodeToString(append(make([]byte, 31), 1))
	keys, err := ValidateKeys([]forkworker.Key{{Name: "a", PublicKey: good}, {Name: "b", PublicKey: other}})
	if err != nil || len(keys) != 2 {
		t.Fatalf("ValidateKeys() = %v, %v", keys, err)
	}
	if len(keys[0].Fingerprint) != 64 {
		t.Fatalf("fingerprint = %q", keys[0].Fingerprint)
	}
	if keys, err := ValidateKeys(nil); err != nil || len(keys) != 0 {
		t.Fatalf("empty allowlist = %v, %v", keys, err)
	}
	bad := map[string][]forkworker.Key{
		"no name":        {{PublicKey: good}},
		"short key":      {{Name: "a", PublicKey: base64.StdEncoding.EncodeToString(make([]byte, 31))}},
		"not base64":     {{Name: "a", PublicKey: strings.Repeat("!", 44)}},
		"duplicate name": {{Name: "a", PublicKey: good}, {Name: "a", PublicKey: other}},
		"duplicate key":  {{Name: "a", PublicKey: good}, {Name: "b", PublicKey: good}},
	}
	for name, k := range bad {
		if _, err := ValidateKeys(k); err == nil {
			t.Errorf("%s: expected an error", name)
		}
	}
}

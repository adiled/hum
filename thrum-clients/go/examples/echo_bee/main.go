// An echo bee: it answers every prompt with its own text. It is the
// smallest whole bee in Go, and the conformance run drives it the same
// way it drives the echo bee in every other language.
//
//	ECHO_HIVE (echo-go), ECHO_AS (main), ECHO_MODEL (echo-go)
//
// It prints what happens to it, one line each: nestled, astray,
// advisory <code>, then closed, refused <why> or failed <why>. A prompt
// whose text is "stray" is answered with a key the thrum does not
// declare; one whose text is "follow" has the bee follow its sigil and
// print sid <chi> for the next tone of it. Closing its stdin closes the
// bee.
package main

import (
	"context"
	"errors"
	"fmt"
	"io"
	"os"

	"github.com/adiled/hum/thrum-clients/go/thrum"
)

func say(line string) { fmt.Println(line) }

func env(key, fallback string) string {
	if v := os.Getenv(key); v != "" {
		return v
	}
	return fallback
}

func main() {
	model := env("ECHO_MODEL", "echo-go")
	bee := thrum.NewBee(thrum.Options{
		Hive: env("ECHO_HIVE", "echo-go"),
		As:   env("ECHO_AS", "main"),
		Role: thrum.RoleNurse,
		Hello: map[string]any{
			"version":  "1.0.0",
			"models":   []string{model},
			"headroom": map[string]any{"freeSlots": 1, "totalSlots": 1},
		},
	})
	bee.On(thrum.ChiPrompt, func(tone thrum.Tone) {
		text, _ := tone["text"].(string)
		sid, _ := tone["sid"].(string)
		if text == "follow" {
			bee.OnSid(sid, func(next thrum.Tone) {
				chi, _ := next["chi"].(string)
				say("sid " + chi)
				bee.OffSid(sid)
			})
		}
		unfurl := thrum.Tone{"chi": thrum.ChiUnfurl, "sid": sid, "petal": 0, "kind": thrum.PetalKindText, "text": "echo: " + text}
		if text == "stray" {
			unfurl["stray"] = true
		}
		_ = bee.Send(unfurl)
		_ = bee.Send(thrum.Tone{"chi": thrum.ChiWilt, "sid": sid, "reason": "stop"})
	})
	bee.OnNestled(func() { say("nestled") })
	bee.OnAstray(func() { say("astray") })
	bee.OnAdvisory(func(a thrum.Advisory) { say("advisory " + a.Code) })

	go func() {
		_, _ = io.Copy(io.Discard, os.Stdin)
		_ = bee.Close()
	}()

	if err := bee.Start(context.Background()); err != nil {
		report(err)
	}
	if err := bee.Wait(); err != nil {
		report(err)
	}
	say("closed")
}

func report(err error) {
	var refused *thrum.Refused
	if errors.As(err, &refused) {
		say("refused " + refused.Reason)
		os.Exit(3)
	}
	say("failed " + err.Error())
	os.Exit(1)
}

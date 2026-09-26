package typesafe

import (
	"context"
	"encoding/json"
	"errors"
	"strings"
	"testing"
)

func marshal(t *testing.T, q Question) string {
	t.Helper()
	b, err := json.Marshal(q)
	if err != nil {
		t.Fatalf("marshal: %v", err)
	}
	return string(b)
}

func TestQuestionSerialization(t *testing.T) {
	t.Parallel()
	cases := []struct {
		name string
		q    Question
		want string
	}{
		{"noul minimal", Noul("Is it spam?"), `{"type":"noul","instructions":"Is it spam?"}`},
		{"noul no instructions", NoulQuestion{}, `{"type":"noul"}`},
		{"noul criteria", Noul("Urgent?").WithCriteria("Time-sensitive", "No urgency"),
			`{"type":"noul","instructions":"Urgent?","criteria":{"true":"Time-sensitive","false":"No urgency"}}`},
		{"noul one-sided criteria", Noul("Urgent?").WithCriteria("Time-sensitive", nil),
			`{"type":"noul","instructions":"Urgent?","criteria":{"true":"Time-sensitive"}}`},
		{"noul empty criteria omitted", Noul("Urgent?").WithCriteria(nil, nil),
			`{"type":"noul","instructions":"Urgent?"}`},
		{"choice preserves order", Choice("Team?", Opt("zeta", "last letter"), Bare("alpha"), Opt("mid", nil)),
			`{"type":"choice","instructions":"Team?","criteria":{"zeta":"last letter","alpha":null,"mid":null}}`},
		{"choice names", ChoiceNames("Tone?", "calm", "angry"),
			`{"type":"choice","instructions":"Tone?","criteria":{"calm":null,"angry":null}}`},
		{"choice map sorted", ChoiceMap("Team?", map[string]string{"sales": "Pricing", "billing": "Payments"}),
			`{"type":"choice","instructions":"Team?","criteria":{"billing":"Payments","sales":"Pricing"}}`},
		{"choice map any with null", ChoiceMap("Team?", map[string]any{"a": nil, "b": map[string]any{"what": "x"}}),
			`{"type":"choice","instructions":"Team?","criteria":{"a":null,"b":{"what":"x"}}}`},
		{"score", Score("Frustration?", "Calm", "Frustrated", "Very angry"),
			`{"type":"score","instructions":"Frustration?","criteria":["Calm","Frustrated","Very angry"]}`},
		{"score structured levels", Score[any]("Scope?", map[string]any{"summary": "One"}, []string{"a", "b"}),
			`{"type":"score","instructions":"Scope?","criteria":[{"summary":"One"},["a","b"]]}`},
		{"structured instructions", Noul(map[string]any{"question": "Same person as `dup`?", "dup": map[string]string{"name": "J"}}),
			"{\"type\":\"noul\",\"instructions\":{\"dup\":{\"name\":\"J\"},\"question\":\"Same person as `dup`?\"}}"},
		{"array instructions", Noul([]string{"Scope first.", "Is it spam?"}),
			`{"type":"noul","instructions":["Scope first.","Is it spam?"]}`},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			t.Parallel()
			if got := marshal(t, tc.q); got != tc.want {
				t.Errorf("got  %s\nwant %s", got, tc.want)
			}
		})
	}
}

func TestScoreFromSlice(t *testing.T) {
	t.Parallel()
	levels := []string{"low", "high"}
	if got := marshal(t, Score("x", levels...)); got != `{"type":"score","instructions":"x","criteria":["low","high"]}` {
		t.Errorf("got %s", got)
	}
}

func TestClientSideValidation(t *testing.T) {
	t.Parallel()
	s := newTestServer(t, okHandler)
	c := newTestClient(t, s)
	cases := []struct {
		name string
		req  Request
		want string
	}{
		{"no questions", Request{State: "x"}, "at least one question"},
		{"nil question", Request{State: "x", Questions: Questions{"a": nil}}, `question "a" is nil`},
		{"empty choice", Request{State: "x", Questions: Questions{"c": Choice("?")}}, `choice question "c" has no options`},
		{"duplicate option", Request{State: "x", Questions: Questions{"c": Choice("?", Bare("a"), Bare("a"))}}, `duplicate option "a"`},
		{"one-level score", Request{State: "x", Questions: Questions{"s": Score("?", "only")}}, `has 1 level(s)`},
		{"null score level", Request{State: "x", Questions: Questions{"s": Score[any]("?", "a", nil)}}, "level 1 is null"},
		{"nil state", Request{Questions: oneQuestion}, "state must be a string"},
		{"number state", Request{State: 42, Questions: oneQuestion}, "state must be a string"},
		{"bool state", Request{State: true, Questions: oneQuestion}, "state must be a string"},
		{"unencodable state", Request{State: make(chan int), Questions: oneQuestion}, "could not be encoded"},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			t.Parallel()
			_, err := c.SystemOne(context.Background(), tc.req)
			if !errors.Is(err, ErrInvalidRequest) {
				t.Fatalf("err = %v, want ErrInvalidRequest", err)
			}
			if !strings.Contains(err.Error(), tc.want) {
				t.Errorf("err = %q, want it to contain %q", err, tc.want)
			}
		})
	}
	if n := len(s.Requests()); n != 0 {
		t.Errorf("server saw %d requests, want 0 (validation must precede I/O)", n)
	}
}

func TestValidStates(t *testing.T) {
	t.Parallel()
	s := newTestServer(t, okHandler)
	c := newTestClient(t, s)
	for _, st := range []any{"text", map[string]any{"a": 1}, []string{"x", "y"}, struct {
		Msg string `json:"msg"`
	}{"hi"}} {
		if _, err := c.SystemOne(context.Background(), Request{State: st, Questions: oneQuestion}); err != nil {
			t.Errorf("state %#v: %v", st, err)
		}
	}
}

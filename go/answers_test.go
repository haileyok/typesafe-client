package typesafe

import (
	"context"
	"errors"
	"net/http"
	"reflect"
	"testing"
)

func TestDecodeAllAnswerKinds(t *testing.T) {
	t.Parallel()
	s := newTestServer(t, func(_ int, _ recorded, w http.ResponseWriter) {
		w.Header().Set("x-typesafe-request-id", "req_abc")
		writeJSON(w, 200, map[string]any{
			"model": "jev-1.13.0",
			"answers": map[string]any{
				"urgent": map[string]any{"type": "noul", "noul": 0},
				"dept": map[string]any{"type": "choice", "choice": "billing",
					"probabilities": map[string]any{"billing": 0.88, "technical": 0.12, "sales": 0.0}, "confidence": 0.81},
				"frustration": map[string]any{"type": "score", "score": 1.05,
					"legend":        map[string]any{"0": "Calm", "1": "Frustrated", "2": map[string]any{"summary": "Very angry"}},
					"probabilities": map[string]any{"0": 0.0, "1": 0.95, "2": 0.05}, "confidence": 0.92},
				"future": map[string]any{"type": "ranking", "order": []string{"a", "b"}},
			},
			"usage": map[string]any{"input_tokens": 304, "output_tokens": 18},
		})
	})
	c := newTestClient(t, s)
	resp, err := c.SystemOne(context.Background(), Request{State: "x", Questions: Questions{
		"urgent":      Noul("Urgent?"),
		"dept":        ChoiceNames("Team?", "billing", "technical", "sales"),
		"frustration": Score("Frustration?", "Calm", "Frustrated", "Very angry"),
	}})
	if err != nil {
		t.Fatal(err)
	}
	if resp.Model != "jev-1.13.0" || resp.RequestID != "req_abc" || resp.Usage != (Usage{304, 18}) {
		t.Errorf("meta: %+v", resp)
	}
	if resp.Header.Get("X-Typesafe-Request-Id") != "req_abc" {
		t.Errorf("header not exposed")
	}
	n, ok := resp.Noul("urgent")
	if !ok || n.Noul != 0 {
		t.Errorf("noul = %+v %v (a genuine 0 must decode)", n, ok)
	}
	ch, ok := resp.Choice("dept")
	if !ok || ch.Choice != "billing" || ch.Confidence != 0.81 || len(ch.Probabilities) != 3 {
		t.Errorf("choice = %+v", ch)
	}
	ranked := ch.Ranked()
	if ranked[0] != (OptionProbability{"billing", 0.88}) || ranked[2].Option != "sales" {
		t.Errorf("ranked = %+v", ranked)
	}
	sc, ok := resp.Score("frustration")
	if !ok || sc.Score != 1.05 || sc.Probabilities[1] != 0.95 {
		t.Errorf("score = %+v", sc)
	}
	if txt, ok := sc.LegendText(1); !ok || txt != "Frustrated" {
		t.Errorf("legend text = %q %v", txt, ok)
	}
	if _, ok := sc.LegendText(2); ok {
		t.Errorf("structured legend reported as text")
	}
	if !reflect.DeepEqual(sc.Legend[2], map[string]any{"summary": "Very angry"}) {
		t.Errorf("structured legend = %#v", sc.Legend[2])
	}
	u, ok := resp.Answers["future"].(UnknownAnswer)
	if !ok || u.Type() != "ranking" || len(u.Raw) == 0 {
		t.Errorf("unknown answer = %#v", resp.Answers["future"])
	}
	if _, ok := resp.Noul("dept"); ok {
		t.Errorf("Noul accessor accepted a choice answer")
	}
	if len(resp.Nouls()) != 1 || len(resp.Choices()) != 1 || len(resp.Scores()) != 1 {
		t.Errorf("typed maps: %d %d %d", len(resp.Nouls()), len(resp.Choices()), len(resp.Scores()))
	}
}

func TestAnswerCompleteness(t *testing.T) {
	t.Parallel()
	questions := Questions{
		"a": Noul("A?"),
		"b": ChoiceNames("B?", "x", "y"),
	}
	noulA := map[string]any{"type": "noul", "noul": 0.2}
	choiceB := map[string]any{"type": "choice", "choice": "x", "probabilities": map[string]any{"x": 1}, "confidence": 1}
	cases := []struct {
		name    string
		answers map[string]any
		path    string // "" = success
	}{
		{"complete", map[string]any{"a": noulA, "b": choiceB}, ""},
		{"extra ids ignored", map[string]any{"a": noulA, "b": choiceB, "zzz": noulA}, ""},
		{"unknown type passes", map[string]any{"a": map[string]any{"type": "future"}, "b": choiceB}, ""},
		{"missing answer", map[string]any{"b": choiceB}, "answers.a"},
		{"type mismatch", map[string]any{"a": choiceB, "b": choiceB}, "answers.a.type"},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			t.Parallel()
			s := newTestServer(t, func(_ int, _ recorded, w http.ResponseWriter) {
				writeJSON(w, 200, map[string]any{"model": "m", "answers": tc.answers})
			})
			c := newTestClient(t, s)
			_, err := c.SystemOne(context.Background(), Request{State: "x", Questions: questions})
			if tc.path == "" {
				if err != nil {
					t.Fatalf("err = %v", err)
				}
				return
			}
			var ve *ResponseValidationError
			if !errors.As(err, &ve) || ve.FieldPath != tc.path {
				t.Fatalf("err = %v, want validation error at %q", err, tc.path)
			}
		})
	}

	// A "questions" extra-body override replaces what was sent, so the
	// completeness check against Request.Questions is skipped.
	s := newTestServer(t, func(_ int, _ recorded, w http.ResponseWriter) {
		writeJSON(w, 200, map[string]any{"model": "m", "answers": map[string]any{"other": noulA}})
	})
	c := newTestClient(t, s)
	if _, err := c.SystemOne(context.Background(), Request{State: "x", Questions: questions},
		RequestExtraBody("questions", map[string]any{"other": map[string]any{"type": "noul"}})); err != nil {
		t.Errorf("override: %v", err)
	}
}

func TestMissingUsageDefaultsToZero(t *testing.T) {
	t.Parallel()
	s := newTestServer(t, func(_ int, _ recorded, w http.ResponseWriter) {
		writeJSON(w, 200, map[string]any{"model": "m", "answers": map[string]any{"q": map[string]any{"type": "noul", "noul": 1}}})
	})
	c := newTestClient(t, s)
	resp, err := c.SystemOne(context.Background(), Request{State: "x", Questions: oneQuestion})
	if err != nil || resp.Usage != (Usage{}) {
		t.Fatalf("resp=%+v err=%v", resp, err)
	}
}

func TestResponseValidation(t *testing.T) {
	t.Parallel()
	cases := []struct {
		name string
		body string
		path string
	}{
		{"not json", `{"answers": [not json`, ""},
		{"array body", `[]`, ""},
		{"missing model", `{"answers":{}}`, "model"},
		{"model wrong type", `{"model":1,"answers":{}}`, "model"},
		{"missing answers", `{"model":"m"}`, "answers"},
		{"answers null", `{"model":"m","answers":null}`, "answers"},
		{"answer not object", `{"model":"m","answers":{"q":1}}`, "answers.q"},
		{"missing type", `{"model":"m","answers":{"q":{}}}`, "answers.q.type"},
		{"noul missing value", `{"model":"m","answers":{"q":{"type":"noul"}}}`, "answers.q.noul"},
		{"noul null value", `{"model":"m","answers":{"q":{"type":"noul","noul":null}}}`, "answers.q.noul"},
		{"choice missing confidence", `{"model":"m","answers":{"tone":{"type":"choice","choice":"a","probabilities":{"a":1}}}}`, "answers.tone.confidence"},
		{"choice bad probabilities", `{"model":"m","answers":{"tone":{"type":"choice","choice":"a","probabilities":{"a":"x"},"confidence":1}}}`, "answers.tone.probabilities"},
		{"score missing legend", `{"model":"m","answers":{"s":{"type":"score","score":1,"confidence":1,"probabilities":{"0":1}}}}`, "answers.s.legend"},
		{"score non-integer key", `{"model":"m","answers":{"s":{"type":"score","score":1,"confidence":1,"legend":{"x":"a"},"probabilities":{"0":1}}}}`, "answers.s.legend"},
		{"usage wrong type", `{"model":"m","answers":{},"usage":{"input_tokens":"many"}}`, "usage.input_tokens"},
		{"usage not object", `{"model":"m","answers":{},"usage":"lots"}`, "usage"},
		{"score bool legend", `{"model":"m","answers":{"s":{"type":"score","score":1,"confidence":1,"legend":{"0":"a","1":true},"probabilities":{"0":1}}}}`, "answers.s.legend.1"},
		{"score number legend", `{"model":"m","answers":{"s":{"type":"score","score":1,"confidence":1,"legend":{"0":3},"probabilities":{"0":1}}}}`, "answers.s.legend.0"},
		{"score null legend", `{"model":"m","answers":{"s":{"type":"score","score":1,"confidence":1,"legend":{"0":null},"probabilities":{"0":1}}}}`, "answers.s.legend.0"},
	}
	for _, tc := range cases {
		t.Run(tc.name, func(t *testing.T) {
			t.Parallel()
			s := newTestServer(t, func(_ int, _ recorded, w http.ResponseWriter) {
				w.Header().Set("x-typesafe-request-id", "req_v")
				w.WriteHeader(200)
				_, _ = w.Write([]byte(tc.body))
			})
			c := newTestClient(t, s)
			_, err := c.SystemOne(context.Background(), Request{State: "x", Questions: oneQuestion})
			var ve *ResponseValidationError
			if !errors.As(err, &ve) {
				t.Fatalf("err = %v (%T), want *ResponseValidationError", err, err)
			}
			if ve.FieldPath != tc.path || ve.StatusCode != 200 || ve.RequestID != "req_v" || string(ve.Body) != tc.body {
				t.Errorf("got %+v, want path %q", ve, tc.path)
			}
			if len(s.Requests()) != 1 {
				t.Errorf("validation errors must not be retried")
			}
		})
	}
}

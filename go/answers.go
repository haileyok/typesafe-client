package typesafe

import (
	"bytes"
	"encoding/json"
	"net/http"
	"sort"
	"strconv"
)

// Answer is one question's typed answer: a NoulAnswer, ChoiceAnswer,
// ScoreAnswer, or, for an answer type this client version does not know,
// an UnknownAnswer. Use a type switch, or the typed accessors on Response.
type Answer interface {
	// Type returns the wire type: "noul", "choice", "score", or the
	// unrecognized type of an UnknownAnswer.
	Type() string
}

// NoulAnswer answers a Noul question.
type NoulAnswer struct {
	// Noul is the probability of yes, from 0 (no) to 1 (yes). Near 0.5 means
	// uncertain, not "medium". Noul answers have no separate confidence.
	Noul float64
}

// Type implements Answer.
func (NoulAnswer) Type() string { return "noul" }

// ChoiceAnswer answers a Choice question.
type ChoiceAnswer struct {
	// Choice is the highest-probability option.
	Choice string
	// Probabilities maps every option to its probability; they sum to
	// approximately 1.
	Probabilities map[string]float64
	// Confidence summarizes how peaked Probabilities is, from 0 to 1. See
	// https://docs.typesafe.ai/confidence.
	Confidence float64
}

// Type implements Answer.
func (ChoiceAnswer) Type() string { return "choice" }

// OptionProbability is one option and its probability.
type OptionProbability struct {
	Option      string
	Probability float64
}

// Ranked returns every option ordered by descending probability (ties by
// option name), useful for beam search or picking runners-up.
func (a ChoiceAnswer) Ranked() []OptionProbability {
	out := make([]OptionProbability, 0, len(a.Probabilities))
	for o, p := range a.Probabilities {
		out = append(out, OptionProbability{Option: o, Probability: p})
	}
	sort.Slice(out, func(i, j int) bool {
		if out[i].Probability != out[j].Probability {
			return out[i].Probability > out[j].Probability
		}
		return out[i].Option < out[j].Option
	})
	return out
}

// ScoreAnswer answers a Score question.
type ScoreAnswer struct {
	// Score is the probability-weighted level, which can fall between
	// levels (for example 1.4 on a 0–2 scale).
	Score float64
	// Confidence summarizes how peaked Probabilities is, from 0 to 1.
	Confidence float64
	// Legend maps each level to its description as sent (a string, or a
	// decoded JSON object or array for structured levels).
	Legend map[int]any
	// Probabilities maps each level to its probability.
	Probabilities map[int]float64
}

// Type implements Answer.
func (ScoreAnswer) Type() string { return "score" }

// LegendText returns the description of level as a string, when it is one.
func (a ScoreAnswer) LegendText(level int) (string, bool) {
	s, ok := a.Legend[level].(string)
	return s, ok
}

// UnknownAnswer carries an answer whose type this client version does not
// model, so newer API answer types do not fail the whole response.
type UnknownAnswer struct {
	// Kind is the answer's "type" field.
	Kind string
	// Raw is the answer's JSON.
	Raw json.RawMessage
}

// Type implements Answer.
func (a UnknownAnswer) Type() string { return a.Kind }

// Usage is the token usage for a request. Only input tokens are billed.
// Fields are zero when the server (or a gateway) did not report them.
type Usage struct {
	InputTokens  int64
	OutputTokens int64
}

// Response is the result of a System One request.
type Response struct {
	// Model is the versioned model that answered (for example "jev-1.13.0"),
	// even when the request named an alias such as "jev-latest". Log it
	// alongside decisions.
	Model string
	// Answers holds one answer per question, keyed by question ID.
	Answers map[string]Answer
	// Usage is the token usage for the request.
	Usage Usage
	// RequestID is the x-typesafe-request-id response header, if present.
	RequestID string
	// Header holds the HTTP response headers.
	Header http.Header
}

// Noul returns the Noul answer for id.
func (r *Response) Noul(id string) (NoulAnswer, bool) {
	a, ok := r.Answers[id].(NoulAnswer)
	return a, ok
}

// Choice returns the Choice answer for id.
func (r *Response) Choice(id string) (ChoiceAnswer, bool) {
	a, ok := r.Answers[id].(ChoiceAnswer)
	return a, ok
}

// Score returns the Score answer for id.
func (r *Response) Score(id string) (ScoreAnswer, bool) {
	a, ok := r.Answers[id].(ScoreAnswer)
	return a, ok
}

// Nouls returns every Noul answer keyed by question ID.
func (r *Response) Nouls() map[string]NoulAnswer { return answersOf[NoulAnswer](r.Answers) }

// Choices returns every Choice answer keyed by question ID.
func (r *Response) Choices() map[string]ChoiceAnswer { return answersOf[ChoiceAnswer](r.Answers) }

// Scores returns every Score answer keyed by question ID.
func (r *Response) Scores() map[string]ScoreAnswer { return answersOf[ScoreAnswer](r.Answers) }

func answersOf[T Answer](all map[string]Answer) map[string]T {
	out := make(map[string]T)
	for id, a := range all {
		if t, ok := a.(T); ok {
			out[id] = t
		}
	}
	return out
}

// ModelMetadata describes one model name accepted by the request's model
// field.
type ModelMetadata struct {
	// Name is the model ID or alias, for example "jev-latest".
	Name string `json:"name"`
	// Description says what the model is for.
	Description string `json:"description"`
	// ReleaseDate is the release date, formatted YYYY-MM-DD.
	ReleaseDate string `json:"release_date"`
}

// ModelList is the response of ListModels.
type ModelList struct {
	Models    []ModelMetadata
	RequestID string
}

// ---- decoding ----

// fieldError is a response validation failure at a dotted field path.
type fieldError struct{ path string }

type object map[string]json.RawMessage

// decodeObject decodes raw as a JSON object; null and non-objects fail.
func decodeObject(raw json.RawMessage, path string) (object, error) {
	if isNull(raw) {
		return nil, &fieldError{path}
	}
	var o object
	if err := json.Unmarshal(raw, &o); err != nil {
		return nil, &fieldError{path}
	}
	return o, nil
}

func (e *fieldError) Error() string { return "invalid response data at " + strconv.Quote(e.path) }

func join(path, key string) string {
	if path == "" {
		return key
	}
	return path + "." + key
}

func isNull(raw json.RawMessage) bool {
	return len(bytes.TrimSpace(raw)) == 0 || bytes.Equal(bytes.TrimSpace(raw), []byte("null"))
}

// required decodes o[key] into dst, failing when it is absent, null, or
// the wrong type.
func required[T any](o object, key, path string, dst *T) error {
	raw, ok := o[key]
	if !ok || isNull(raw) {
		return &fieldError{join(path, key)}
	}
	if err := json.Unmarshal(raw, dst); err != nil {
		return &fieldError{join(path, key)}
	}
	return nil
}

// optional decodes o[key] into dst when present and non-null.
func optional[T any](o object, key, path string, dst *T) error {
	raw, ok := o[key]
	if !ok || isNull(raw) {
		return nil
	}
	if err := json.Unmarshal(raw, dst); err != nil {
		return &fieldError{join(path, key)}
	}
	return nil
}

// checkComplete requires an answer for every requested question, and that
// an answer of a known type matches its question's type. Unknown (newer)
// answer types and extra answer IDs pass through.
func checkComplete(resp *Response, questions Questions) error {
	ids := make([]string, 0, len(questions))
	for id := range questions {
		ids = append(ids, id)
	}
	sort.Strings(ids) // report the first problem deterministically
	for _, id := range ids {
		a, ok := resp.Answers[id]
		if !ok {
			return &fieldError{join("answers", id)}
		}
		if _, unknown := a.(UnknownAnswer); !unknown && a.Type() != questions[id].Type() {
			return &fieldError{join(join("answers", id), "type")}
		}
	}
	return nil
}

func decodeSystemOne(body []byte) (*Response, error) {
	top, err := decodeObject(body, "")
	if err != nil {
		return nil, err
	}
	out := &Response{}
	if err := required(top, "model", "", &out.Model); err != nil {
		return nil, err
	}
	rawAnswers, ok := top["answers"]
	if !ok {
		return nil, &fieldError{"answers"}
	}
	answers, err := decodeObject(rawAnswers, "answers")
	if err != nil {
		return nil, err
	}
	out.Answers = make(map[string]Answer, len(answers))
	for id, raw := range answers {
		a, err := decodeAnswer(raw, join("answers", id))
		if err != nil {
			return nil, err
		}
		out.Answers[id] = a
	}
	if raw, ok := top["usage"]; ok && !isNull(raw) {
		u, err := decodeObject(raw, "usage")
		if err != nil {
			return nil, err
		}
		if err := optional(u, "input_tokens", "usage", &out.Usage.InputTokens); err != nil {
			return nil, err
		}
		if err := optional(u, "output_tokens", "usage", &out.Usage.OutputTokens); err != nil {
			return nil, err
		}
	}
	return out, nil
}

func decodeAnswer(raw json.RawMessage, path string) (Answer, error) {
	o, err := decodeObject(raw, path)
	if err != nil {
		return nil, err
	}
	var kind string
	if err := required(o, "type", path, &kind); err != nil {
		return nil, err
	}
	switch kind {
	case "noul":
		var a NoulAnswer
		if err := required(o, "noul", path, &a.Noul); err != nil {
			return nil, err
		}
		return a, nil
	case "choice":
		var a ChoiceAnswer
		if err := required(o, "choice", path, &a.Choice); err != nil {
			return nil, err
		}
		if err := required(o, "probabilities", path, &a.Probabilities); err != nil {
			return nil, err
		}
		if err := required(o, "confidence", path, &a.Confidence); err != nil {
			return nil, err
		}
		return a, nil
	case "score":
		var a ScoreAnswer
		if err := required(o, "score", path, &a.Score); err != nil {
			return nil, err
		}
		if err := required(o, "confidence", path, &a.Confidence); err != nil {
			return nil, err
		}
		// encoding/json decodes decimal string keys into int map keys and
		// rejects anything else.
		if err := required(o, "legend", path, &a.Legend); err != nil {
			return nil, err
		}
		// Legend values are string, object, or array (OpenAPI
		// ScoreAnswer.legend); null, booleans, and numbers are invalid.
		for level, v := range a.Legend {
			switch v.(type) {
			case string, map[string]any, []any:
			default:
				return nil, &fieldError{join(join(path, "legend"), strconv.Itoa(level))}
			}
		}
		if err := required(o, "probabilities", path, &a.Probabilities); err != nil {
			return nil, err
		}
		return a, nil
	default:
		return UnknownAnswer{Kind: kind, Raw: append(json.RawMessage(nil), raw...)}, nil
	}
}

func decodeModels(body []byte) ([]ModelMetadata, error) {
	top, err := decodeObject(body, "")
	if err != nil {
		return nil, err
	}
	raw, ok := top["models"]
	if !ok || isNull(raw) {
		return nil, &fieldError{"models"}
	}
	var entries []json.RawMessage
	if err := json.Unmarshal(raw, &entries); err != nil {
		return nil, &fieldError{"models"}
	}
	out := make([]ModelMetadata, len(entries))
	for i, e := range entries {
		path := "models[" + strconv.Itoa(i) + "]"
		o, err := decodeObject(e, path)
		if err != nil {
			return nil, err
		}
		if err := required(o, "name", path, &out[i].Name); err != nil {
			return nil, err
		}
		if err := required(o, "description", path, &out[i].Description); err != nil {
			return nil, err
		}
		if err := required(o, "release_date", path, &out[i].ReleaseDate); err != nil {
			return nil, err
		}
	}
	return out, nil
}

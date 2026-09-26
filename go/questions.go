package typesafe

import (
	"bytes"
	"encoding/json"
	"fmt"
	"sort"
)

// Question is one typed question evaluated against a state. It is
// implemented only by NoulQuestion, ChoiceQuestion, and ScoreQuestion; build
// them with Noul, Choice, ChoiceMap, ChoiceNames, and Score.
//
// Every question has optional instructions: the judgment to make, written as
// a complete question or statement. Question IDs (the keys of Questions) are
// not sent to the model, so put the full meaning in the instructions.
//
// Instructions and criteria descriptions accept a string, or any value that
// marshals to a JSON object or array (map, struct, slice), which lets a
// question carry structured context. See
// https://docs.typesafe.ai/primitives/advanced.
type Question interface {
	json.Marshaler
	// Type returns the wire type: "noul", "choice", or "score".
	Type() string
	validate(id string) error
}

// Questions maps question IDs, chosen by you, to questions. Answers come
// back under the same IDs.
type Questions map[string]Question

// NoulQuestion is a yes/no question. Its answer is the probability that the
// answer is yes. See https://docs.typesafe.ai/primitives/noul.
type NoulQuestion struct {
	// Instructions is the yes/no question or statement; nil omits it.
	Instructions any
	// Criteria optionally describes what yes and no mean; nil omits it.
	Criteria *NoulCriteria
}

// NoulCriteria describes the yes and no outcomes of a Noul. A nil field is
// omitted from the wire.
type NoulCriteria struct {
	True  any
	False any
}

// Noul builds a yes/no question.
func Noul(instructions any) NoulQuestion {
	return NoulQuestion{Instructions: instructions}
}

// WithCriteria returns a copy of q with descriptions of the yes (trueDesc)
// and no (falseDesc) outcomes. Pass nil to leave one side undescribed.
func (q NoulQuestion) WithCriteria(trueDesc, falseDesc any) NoulQuestion {
	q.Criteria = &NoulCriteria{True: trueDesc, False: falseDesc}
	return q
}

// Type implements Question.
func (NoulQuestion) Type() string { return "noul" }

func (q NoulQuestion) validate(string) error { return nil }

// MarshalJSON implements json.Marshaler.
func (q NoulQuestion) MarshalJSON() ([]byte, error) {
	w := newObjectWriter()
	w.field("type", "noul")
	if q.Instructions != nil {
		w.field("instructions", q.Instructions)
	}
	if q.Criteria != nil && (q.Criteria.True != nil || q.Criteria.False != nil) {
		c := newObjectWriter()
		if q.Criteria.True != nil {
			c.field("true", q.Criteria.True)
		}
		if q.Criteria.False != nil {
			c.field("false", q.Criteria.False)
		}
		raw, err := c.bytes()
		if err != nil {
			return nil, err
		}
		w.field("criteria", json.RawMessage(raw))
	}
	return w.bytes()
}

// ChoiceOption is one Choice option: a name and an optional description. A nil
// Description is sent as JSON null, meaning the option is interpreted by its
// name alone.
type ChoiceOption struct {
	Name        string
	Description any
}

// Opt builds a described Choice option. The description may be a string or
// a structured value (for example what the option covers, what it does not,
// and examples).
func Opt(name string, description any) ChoiceOption {
	return ChoiceOption{Name: name, Description: description}
}

// Bare builds a Choice option with no description.
func Bare(name string) ChoiceOption {
	return ChoiceOption{Name: name}
}

// ChoiceQuestion picks one option from a set you define. Its answer is the
// chosen option, a probability per option, and a confidence. Options are
// sent in slice order. See https://docs.typesafe.ai/primitives/choice.
type ChoiceQuestion struct {
	// Instructions is what the model should decide; nil omits it.
	Instructions any
	// Criteria is the ordered option list. At least one option is required;
	// the API accepts up to 255.
	Criteria []ChoiceOption
}

// Choice builds a Choice question from options in the order given.
func Choice(instructions any, options ...ChoiceOption) ChoiceQuestion {
	return ChoiceQuestion{Instructions: instructions, Criteria: options}
}

// ChoiceNames builds a Choice question whose options are described by
// their names alone, in the order given.
func ChoiceNames(instructions any, names ...string) ChoiceQuestion {
	opts := make([]ChoiceOption, len(names))
	for i, n := range names {
		opts[i] = Bare(n)
	}
	return ChoiceQuestion{Instructions: instructions, Criteria: opts}
}

// ChoiceMap builds a Choice question from a map of option name to
// description. Go maps are unordered, so options are sorted by name for a
// deterministic request; use Choice to control the order.
func ChoiceMap[V any](instructions any, criteria map[string]V) ChoiceQuestion {
	names := make([]string, 0, len(criteria))
	for n := range criteria {
		names = append(names, n)
	}
	sort.Strings(names)
	opts := make([]ChoiceOption, len(names))
	for i, n := range names {
		opts[i] = Opt(n, any(criteria[n]))
	}
	return ChoiceQuestion{Instructions: instructions, Criteria: opts}
}

// Type implements Question.
func (ChoiceQuestion) Type() string { return "choice" }

func (q ChoiceQuestion) validate(id string) error {
	if len(q.Criteria) == 0 {
		return invalidf("choice question %q has no options; at least one is required", id)
	}
	seen := make(map[string]struct{}, len(q.Criteria))
	for _, o := range q.Criteria {
		if _, dup := seen[o.Name]; dup {
			return invalidf("choice question %q has duplicate option %q", id, o.Name)
		}
		seen[o.Name] = struct{}{}
	}
	return nil
}

// MarshalJSON implements json.Marshaler. Options are written in order.
func (q ChoiceQuestion) MarshalJSON() ([]byte, error) {
	c := newObjectWriter()
	for _, o := range q.Criteria {
		c.field(o.Name, o.Description)
	}
	criteria, err := c.bytes()
	if err != nil {
		return nil, err
	}
	w := newObjectWriter()
	w.field("type", "choice")
	if q.Instructions != nil {
		w.field("instructions", q.Instructions)
	}
	w.field("criteria", json.RawMessage(criteria))
	return w.bytes()
}

// ScoreQuestion rates the state along ordered levels, lowest first. Its
// answer is a probability-weighted position (which may fall between
// levels), a probability per level, and a confidence. See
// https://docs.typesafe.ai/primitives/score.
type ScoreQuestion struct {
	// Instructions is what the model should rate; nil omits it.
	Instructions any
	// Criteria is the ordered level descriptions; level i scores i. At least
	// two levels are required; the API accepts up to 10. Levels may not be
	// nil.
	Criteria []any
}

// Score builds a Score question from level descriptions, lowest first.
//
//	typesafe.Score("How frustrated is the customer?", "Calm", "Frustrated", "Very angry")
func Score[V any](instructions any, levels ...V) ScoreQuestion {
	c := make([]any, len(levels))
	for i, l := range levels {
		c[i] = l
	}
	return ScoreQuestion{Instructions: instructions, Criteria: c}
}

// Type implements Question.
func (ScoreQuestion) Type() string { return "score" }

func (q ScoreQuestion) validate(id string) error {
	if len(q.Criteria) < 2 {
		return invalidf("score question %q has %d level(s); at least two are required", id, len(q.Criteria))
	}
	for i, l := range q.Criteria {
		raw, err := json.Marshal(l)
		if err != nil {
			return invalidf("score question %q level %d: %v", id, i, err)
		}
		if bytes.Equal(raw, []byte("null")) {
			return invalidf("score question %q level %d is null; every level needs a description", id, i)
		}
	}
	return nil
}

// MarshalJSON implements json.Marshaler.
func (q ScoreQuestion) MarshalJSON() ([]byte, error) {
	w := newObjectWriter()
	w.field("type", "score")
	if q.Instructions != nil {
		w.field("instructions", q.Instructions)
	}
	levels := q.Criteria
	if levels == nil {
		levels = []any{}
	}
	w.field("criteria", levels)
	return w.bytes()
}

// objectWriter writes a JSON object with keys in insertion order.
type objectWriter struct {
	buf bytes.Buffer
	n   int
	err error
}

func newObjectWriter() *objectWriter {
	w := &objectWriter{}
	w.buf.WriteByte('{')
	return w
}

func (w *objectWriter) field(key string, value any) {
	if w.err != nil {
		return
	}
	k, err := json.Marshal(key)
	if err != nil {
		w.err = err
		return
	}
	v, err := json.Marshal(value)
	if err != nil {
		w.err = fmt.Errorf("field %s: %w", k, err)
		return
	}
	if w.n > 0 {
		w.buf.WriteByte(',')
	}
	w.buf.Write(k)
	w.buf.WriteByte(':')
	w.buf.Write(v)
	w.n++
}

func (w *objectWriter) bytes() ([]byte, error) {
	if w.err != nil {
		return nil, w.err
	}
	w.buf.WriteByte('}')
	return w.buf.Bytes(), nil
}

//! Runtime values (spec section 4).
//!
//! Jevscript is dynamically typed with a small fixed set of types. The compiler
//! checks what it can from declarations; everything else is checked here and
//! raises [`crate::error::RuntimeErrorCode::TypeError`] on mismatch.
//!
//! Values cross two boundaries with two different JSON forms:
//!
//! - **The host form** (`Serialize` / `Deserialize`). Hosts pass inputs and
//!   adapter results as plain JSON, so `none`, `text`, `number`, `bool`, `list`
//!   and `record` are exactly `null`, a string, a number, a bool, an array and
//!   an object. The structured kinds are objects carrying a `$jev`
//!   discriminator, such as `{"$jev": "prob", "value": 0.8}`. `$` cannot appear
//!   in an identifier (spec section 2.4), so no record a program builds can
//!   collide with it, and an object without `$jev` is always a record.
//! - **The plain form** ([`Value::to_json`]), which is what Jev sees in a state
//!   and what the text form of a list or record prints (spec section 4.2). A
//!   `prob` is its number, a `choice` is `{label, confidence, probabilities}`,
//!   and so on: no discriminator, nothing a model has to unlearn.

use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::collections::BTreeMap;

use crate::error::{RuntimeError, RuntimeErrorCode};

/// The discriminator key of a structured value in the host form.
pub const JEV_TAG: &str = "$jev";

/// The result of a `pick` (spec section 6.3) or a `pick among` (6.4a).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Choice {
    /// The chosen label. For a `pick among` this is `"i<index>"` or `"none"`.
    pub label: String,
    /// How peaked the distribution is.
    pub confidence: f64,
    /// Label to probability.
    pub probabilities: BTreeMap<String, f64>,
    /// `pick among` only: the chosen element's position in the list.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub index: Option<u32>,
    /// `pick among` only: the chosen element itself.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub item: Option<Box<Value>>,
}

/// The result of a `rate` (spec section 6.4).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Level {
    /// The nearest whole level index, starting at 0.
    pub level: u32,
    /// The probability-weighted mean.
    pub score: f64,
    /// `score / (levels - 1)`.
    pub normalized: f64,
    /// How peaked the distribution is.
    pub confidence: f64,
    /// Level name, or index as text, to probability.
    pub probabilities: BTreeMap<String, f64>,
    /// The level names, if the levels were named. Enables `l is unchanged`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub names: Vec<String>,
}

/// An opaque reference to something an adapter owns, such as a spawned agent or
/// a worktree.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Handle {
    /// The capability that minted it.
    pub capability: String,
    /// The adapter's own identifier for it.
    pub id: String,
    /// Anything else the adapter attached.
    #[serde(default, flatten)]
    pub fields: BTreeMap<String, serde_json::Value>,
}

/// What a `confirm` pause returned from the host.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PauseResult {
    /// Which option the host chose.
    pub answer: String,
    /// Free text the host added, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
}

/// A runtime value.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    /// The absent value.
    None,
    /// An immutable Unicode string.
    Text(String),
    /// An IEEE double. There is no integer/float distinction.
    Number(f64),
    /// `true` or `false`.
    Bool(bool),
    /// An ordered sequence.
    List(Vec<Value>),
    /// An ordered map from identifier to value.
    Record(BTreeMap<String, Value>),
    /// A number in `[0, 1]` produced by `feels`. Behaves as a number in
    /// arithmetic and comparison.
    Prob(f64),
    /// The result of a `pick`.
    Choice(Choice),
    /// The result of a `rate`.
    Level(Level),
    /// An adapter's handle.
    Handle(Handle),
    /// What a `confirm` pause returned.
    PauseResult(PauseResult),
}

impl Value {
    /// Truthiness (spec section 4.1).
    ///
    /// `false`, `none`, `0`, `""`, `[]` and `{}` are false; everything else is
    /// true. A `prob` is a number, so a bare `prob` in a condition tests
    /// `p != 0`, which is almost never what a program means — the compiler warns
    /// about it.
    pub fn is_truthy(&self) -> bool {
        match self {
            Value::None => false,
            Value::Bool(b) => *b,
            Value::Number(n) | Value::Prob(n) => *n != 0.0,
            Value::Text(t) => !t.is_empty(),
            Value::List(items) => !items.is_empty(),
            Value::Record(fields) => !fields.is_empty(),
            Value::Choice(_) | Value::Level(_) | Value::Handle(_) | Value::PauseResult(_) => true,
        }
    }

    /// The name of this value's type, as the spec writes it.
    pub const fn type_name(&self) -> &'static str {
        match self {
            Value::None => "none",
            Value::Text(_) => "text",
            Value::Number(_) => "number",
            Value::Bool(_) => "bool",
            Value::List(_) => "list",
            Value::Record(_) => "record",
            Value::Prob(_) => "prob",
            Value::Choice(_) => "choice",
            Value::Level(_) => "level",
            Value::Handle(_) => "handle",
            Value::PauseResult(_) => "pause_result",
        }
    }

    /// The text form used by interpolation and by capability calls that take
    /// text (spec section 4.2).
    ///
    /// Numbers print without trailing zeros, lists and records print as JSON in
    /// the plain form, a `choice` prints its label, a `level` prints its level
    /// index, and `none` prints as empty text.
    pub fn to_text(&self) -> String {
        match self {
            Value::None => String::new(),
            Value::Text(text) => text.clone(),
            Value::Number(n) | Value::Prob(n) => format_number(*n),
            Value::Bool(b) => b.to_string(),
            Value::Choice(choice) => choice.label.clone(),
            Value::Level(level) => level.level.to_string(),
            Value::List(_) | Value::Record(_) | Value::Handle(_) | Value::PauseResult(_) => {
                self.to_json().to_string()
            }
        }
    }

    /// `==` (spec section 4.3): equality by value.
    ///
    /// A `prob` is a number, so `p == 0.5` compares the numbers. Everything
    /// else is equal only to a value of the same type with the same contents.
    pub fn equals(&self, other: &Value) -> bool {
        match (self, other) {
            (Value::Number(a) | Value::Prob(a), Value::Number(b) | Value::Prob(b)) => a == b,
            (Value::List(a), Value::List(b)) => {
                a.len() == b.len() && a.iter().zip(b).all(|(x, y)| x.equals(y))
            }
            (Value::Record(a), Value::Record(b)) => {
                a.len() == b.len() && a.iter().all(|(k, x)| b.get(k).is_some_and(|y| x.equals(y)))
            }
            _ => self == other,
        }
    }

    /// The numeric value of a `number` or a `prob`.
    pub const fn as_number(&self) -> Option<f64> {
        match self {
            Value::Number(n) | Value::Prob(n) => Some(*n),
            _ => None,
        }
    }

    /// The plain JSON form: what Jev sees in a state, and what a list or record
    /// prints as (spec sections 4.2 and 6.9).
    ///
    /// The structured kinds lose their discriminator: a `prob` is its number, a
    /// `choice` is `{label, confidence, probabilities}` (plus `index` and `item`
    /// from a `pick among`), a `level` is `{level, score, normalized,
    /// confidence, probabilities}`, a `handle` is `{capability, id, ...}` and a
    /// `pause_result` is `{answer, text}`.
    pub fn to_json(&self) -> serde_json::Value {
        use serde_json::{Map, Number, Value as Json};
        match self {
            Value::None => Json::Null,
            Value::Text(text) => Json::String(text.clone()),
            Value::Number(n) | Value::Prob(n) => number_json(*n),
            Value::Bool(b) => Json::Bool(*b),
            Value::List(items) => Json::Array(items.iter().map(Value::to_json).collect()),
            Value::Record(fields) => Json::Object(
                fields
                    .iter()
                    .map(|(k, v)| (k.clone(), v.to_json()))
                    .collect(),
            ),
            Value::Choice(choice) => {
                let mut map = Map::new();
                map.insert("label".into(), Json::String(choice.label.clone()));
                map.insert("confidence".into(), number_json(choice.confidence));
                map.insert(
                    "probabilities".into(),
                    probabilities_json(&choice.probabilities),
                );
                if let Some(index) = choice.index {
                    map.insert("index".into(), Json::Number(Number::from(index)));
                }
                if let Some(item) = &choice.item {
                    map.insert("item".into(), item.to_json());
                }
                Json::Object(map)
            }
            Value::Level(level) => {
                let mut map = Map::new();
                map.insert("level".into(), Json::Number(Number::from(level.level)));
                map.insert("score".into(), number_json(level.score));
                map.insert("normalized".into(), number_json(level.normalized));
                map.insert("confidence".into(), number_json(level.confidence));
                map.insert(
                    "probabilities".into(),
                    probabilities_json(&level.probabilities),
                );
                Json::Object(map)
            }
            Value::Handle(handle) => {
                let mut map = Map::new();
                map.insert("capability".into(), Json::String(handle.capability.clone()));
                map.insert("id".into(), Json::String(handle.id.clone()));
                for (k, v) in &handle.fields {
                    map.entry(k.clone()).or_insert_with(|| v.clone());
                }
                Json::Object(map)
            }
            Value::PauseResult(result) => {
                let mut map = Map::new();
                map.insert("answer".into(), Json::String(result.answer.clone()));
                map.insert(
                    "text".into(),
                    result
                        .text
                        .as_ref()
                        .map_or(Json::Null, |t| Json::String(t.clone())),
                );
                Json::Object(map)
            }
        }
    }

    /// A value from plain JSON: `null`, strings, numbers, bools, arrays and
    /// objects become `none`, `text`, `number`, `bool`, `list` and `record`.
    ///
    /// This is the inverse of [`Value::to_json`] for the plain kinds only. An
    /// object is always a record here, even one carrying `$jev`; the host form
    /// with its discriminator is what `Deserialize` reads.
    pub fn from_json(json: &serde_json::Value) -> Value {
        use serde_json::Value as Json;
        match json {
            Json::Null => Value::None,
            Json::Bool(b) => Value::Bool(*b),
            Json::Number(n) => Value::Number(n.as_f64().unwrap_or(f64::NAN)),
            Json::String(s) => Value::Text(s.clone()),
            Json::Array(items) => Value::List(items.iter().map(Value::from_json).collect()),
            Json::Object(fields) => Value::Record(
                fields
                    .iter()
                    .map(|(k, v)| (k.clone(), Value::from_json(v)))
                    .collect(),
            ),
        }
    }

    /// The host form: plain JSON for the plain kinds, and a `$jev`-tagged object
    /// for the structured kinds. This is what `Serialize` writes.
    pub fn to_host_json(&self) -> serde_json::Value {
        use serde_json::Value as Json;
        let tag = |mut json: Json, kind: &str| {
            if let Json::Object(map) = &mut json {
                map.insert(JEV_TAG.to_string(), Json::String(kind.to_string()));
            }
            json
        };
        match self {
            Value::None | Value::Text(_) | Value::Number(_) | Value::Bool(_) => self.to_json(),
            Value::List(items) => Json::Array(items.iter().map(Value::to_host_json).collect()),
            Value::Record(fields) => Json::Object(
                fields
                    .iter()
                    .map(|(k, v)| (k.clone(), v.to_host_json()))
                    .collect(),
            ),
            Value::Prob(p) => {
                let mut map = serde_json::Map::new();
                map.insert("value".into(), number_json(*p));
                tag(Json::Object(map), "prob")
            }
            Value::Choice(choice) => {
                let mut json = self.to_json();
                if let (Json::Object(map), Some(item)) = (&mut json, &choice.item) {
                    map.insert("item".into(), item.to_host_json());
                }
                tag(json, "choice")
            }
            Value::Level(level) => {
                let mut json = self.to_json();
                if let Json::Object(map) = &mut json
                    && !level.names.is_empty()
                {
                    map.insert(
                        "names".into(),
                        Json::Array(level.names.iter().cloned().map(Json::String).collect()),
                    );
                }
                tag(json, "level")
            }
            Value::Handle(_) => tag(self.to_json(), "handle"),
            Value::PauseResult(_) => tag(self.to_json(), "pause_result"),
        }
    }

    /// A value from the host form. This is what `Deserialize` reads.
    ///
    /// # Errors
    ///
    /// If a `$jev` object names an unknown kind or lacks a required field.
    pub fn from_host_json(json: &serde_json::Value) -> Result<Value, String> {
        use serde_json::Value as Json;
        let Json::Object(fields) = json else {
            return Ok(match json {
                Json::Array(items) => Value::List(
                    items
                        .iter()
                        .map(Value::from_host_json)
                        .collect::<Result<_, _>>()?,
                ),
                other => Value::from_json(other),
            });
        };
        let Some(Json::String(kind)) = fields.get(JEV_TAG) else {
            return Ok(Value::Record(
                fields
                    .iter()
                    .map(|(k, v)| Ok((k.clone(), Value::from_host_json(v)?)))
                    .collect::<Result<_, String>>()?,
            ));
        };
        let number = |key: &str| -> Result<f64, String> {
            fields
                .get(key)
                .and_then(Json::as_f64)
                .ok_or_else(|| format!("a `{kind}` value needs a numeric `{key}`"))
        };
        let text = |key: &str| -> Result<String, String> {
            fields
                .get(key)
                .and_then(Json::as_str)
                .map(str::to_string)
                .ok_or_else(|| format!("a `{kind}` value needs a text `{key}`"))
        };
        let probabilities = || -> Result<BTreeMap<String, f64>, String> {
            match fields.get("probabilities") {
                Some(Json::Object(map)) => map
                    .iter()
                    .map(|(k, v)| {
                        v.as_f64()
                            .map(|p| (k.clone(), p))
                            .ok_or_else(|| format!("probability `{k}` is not a number"))
                    })
                    .collect(),
                None => Ok(BTreeMap::new()),
                Some(_) => Err(format!("a `{kind}` value needs a `probabilities` object")),
            }
        };
        match kind.as_str() {
            "prob" => Ok(Value::Prob(number("value")?)),
            "choice" => Ok(Value::Choice(Choice {
                label: text("label")?,
                confidence: number("confidence").unwrap_or(0.0),
                probabilities: probabilities()?,
                index: fields
                    .get("index")
                    .and_then(Json::as_u64)
                    .and_then(|i| u32::try_from(i).ok()),
                item: match fields.get("item") {
                    Some(item) if !item.is_null() => Some(Box::new(Value::from_host_json(item)?)),
                    _ => None,
                },
            })),
            "level" => Ok(Value::Level(Level {
                level: fields
                    .get("level")
                    .and_then(Json::as_u64)
                    .and_then(|i| u32::try_from(i).ok())
                    .ok_or("a `level` value needs a whole `level`")?,
                score: number("score")?,
                normalized: number("normalized").unwrap_or(0.0),
                confidence: number("confidence").unwrap_or(0.0),
                probabilities: probabilities()?,
                names: match fields.get("names") {
                    Some(Json::Array(names)) => names
                        .iter()
                        .map(|n| n.as_str().map(str::to_string))
                        .collect::<Option<Vec<_>>>()
                        .ok_or("level `names` must be texts")?,
                    _ => Vec::new(),
                },
            })),
            "handle" => Ok(Value::Handle(Handle {
                capability: text("capability")?,
                id: text("id")?,
                fields: fields
                    .iter()
                    .filter(|(k, _)| !matches!(k.as_str(), "capability" | "id") && *k != JEV_TAG)
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect(),
            })),
            "pause_result" => Ok(Value::PauseResult(PauseResult {
                answer: text("answer")?,
                text: fields
                    .get("text")
                    .and_then(Json::as_str)
                    .map(str::to_string),
            })),
            other => Err(format!("unknown `{JEV_TAG}` kind `{other}`")),
        }
    }

    /// A `type_error` (spec section 12) saying what was expected and what the
    /// value actually was.
    pub fn type_error(expected: &str, got: &Value) -> RuntimeError {
        RuntimeError::new(
            RuntimeErrorCode::TypeError,
            format!("expected {expected}, got {}", got.type_name()),
        )
    }
}

impl Serialize for Value {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.to_host_json().serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for Value {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let json = serde_json::Value::deserialize(deserializer)?;
        Value::from_host_json(&json).map_err(D::Error::custom)
    }
}

impl From<serde_json::Value> for Value {
    /// Plain JSON in, plain kinds out: the same mapping as [`Value::from_json`].
    fn from(json: serde_json::Value) -> Self {
        Value::from_json(&json)
    }
}

/// A JSON number for an `f64`. A whole number is written without a fraction,
/// matching the text form (spec section 4.2). A non-finite double has no JSON
/// form and becomes `null`, which is the only honest rendering.
fn number_json(n: f64) -> serde_json::Value {
    if n.is_finite() && n.fract() == 0.0 && n.abs() < 9.007_199_254_740_992e15 {
        return serde_json::Value::Number(serde_json::Number::from(n as i64));
    }
    serde_json::Number::from_f64(n).map_or(serde_json::Value::Null, serde_json::Value::Number)
}

fn probabilities_json(probabilities: &BTreeMap<String, f64>) -> serde_json::Value {
    serde_json::Value::Object(
        probabilities
            .iter()
            .map(|(k, p)| (k.clone(), number_json(*p)))
            .collect(),
    )
}

/// Numbers print without trailing zeros (spec section 4.2).
fn format_number(n: f64) -> String {
    if n.is_finite() && n.fract() == 0.0 && n.abs() < 1e15 {
        format!("{n:.0}")
    } else {
        format!("{n}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn choice() -> Choice {
        Choice {
            label: "stuck".to_string(),
            confidence: 0.8,
            probabilities: BTreeMap::from([("stuck".to_string(), 0.8), ("other".to_string(), 0.2)]),
            index: None,
            item: None,
        }
    }

    #[test]
    fn empty_things_are_false() {
        // Spec 4.1.
        assert!(!Value::None.is_truthy());
        assert!(!Value::Text(String::new()).is_truthy());
        assert!(!Value::Number(0.0).is_truthy());
        assert!(!Value::List(Vec::new()).is_truthy());
        assert!(!Value::Record(BTreeMap::new()).is_truthy());
        assert!(!Value::Bool(false).is_truthy());
        assert!(Value::Text("x".into()).is_truthy());
        assert!(Value::Choice(choice()).is_truthy());
    }

    #[test]
    fn a_prob_is_a_number_in_a_condition() {
        // Section 4.1: this is why the compiler warns on a bare prob.
        assert!(Value::Prob(0.01).is_truthy());
        assert!(!Value::Prob(0.0).is_truthy());
    }

    #[test]
    fn a_pick_among_choice_carries_its_index_and_item() {
        // Spec 6.4a: `label` is `"i<index>"` and the program acts on `item`.
        let choice = Choice {
            label: "i2".to_string(),
            confidence: 0.7,
            probabilities: BTreeMap::from([("i2".to_string(), 0.7)]),
            index: Some(2),
            item: Some(Box::new(Value::Text("the third candidate".to_string()))),
        };
        assert_eq!(Value::Choice(choice.clone()).to_text(), "i2");
        assert_eq!(choice.index, Some(2));
    }

    #[test]
    fn numbers_print_without_trailing_zeros() {
        // Spec 4.2.
        assert_eq!(Value::Number(3.0).to_text(), "3");
        assert_eq!(Value::Number(0.75).to_text(), "0.75");
        assert_eq!(Value::Number(2000.0).to_text(), "2000");
        assert_eq!(Value::Prob(0.5).to_text(), "0.5");
    }

    #[test]
    fn none_prints_as_empty_text_and_a_choice_prints_its_label() {
        // Spec 4.2.
        assert_eq!(Value::None.to_text(), "");
        assert_eq!(Value::Choice(choice()).to_text(), "stuck");
        assert_eq!(
            Value::Level(Level {
                level: 2,
                score: 1.7,
                normalized: 0.85,
                confidence: 0.6,
                probabilities: BTreeMap::new(),
                names: vec![],
            })
            .to_text(),
            "2"
        );
    }

    #[test]
    fn lists_and_records_print_as_plain_json() {
        // Spec 4.2: JSON, and the plain form, so a prob inside prints as its
        // number rather than as a tagged object.
        let record = Value::Record(BTreeMap::from([
            ("p".to_string(), Value::Prob(0.25)),
            (
                "xs".to_string(),
                Value::List(vec![Value::Number(1.0), Value::None]),
            ),
        ]));
        assert_eq!(record.to_text(), r#"{"p":0.25,"xs":[1,null]}"#);
    }

    #[test]
    fn equality_is_by_value_and_a_prob_is_a_number() {
        // Spec 4.3.
        assert!(Value::Prob(0.5).equals(&Value::Number(0.5)));
        assert!(!Value::Number(1.0).equals(&Value::Text("1".into())));
        assert!(
            Value::List(vec![Value::Number(1.0), Value::Text("a".into())]).equals(&Value::List(
                vec![Value::Prob(1.0), Value::Text("a".into())]
            ))
        );
        assert!(
            Value::Record(BTreeMap::from([("a".to_string(), Value::Number(1.0))])).equals(
                &Value::Record(BTreeMap::from([("a".to_string(), Value::Prob(1.0))]))
            )
        );
        assert!(!Value::None.equals(&Value::Bool(false)));
        assert!(Value::Choice(choice()).equals(&Value::Choice(choice())));
    }

    #[test]
    fn plain_json_round_trips_through_the_host_form() {
        // Hosts pass inputs as plain JSON: an object without `$jev` is a record.
        let json = json!({"title": "x", "n": 3, "ok": true, "gone": null, "xs": [1, "two"]});
        let value: Value = serde_json::from_value(json.clone()).expect("plain JSON parses");
        assert_eq!(
            value,
            Value::Record(BTreeMap::from([
                ("title".to_string(), Value::Text("x".into())),
                ("n".to_string(), Value::Number(3.0)),
                ("ok".to_string(), Value::Bool(true)),
                ("gone".to_string(), Value::None),
                (
                    "xs".to_string(),
                    Value::List(vec![Value::Number(1.0), Value::Text("two".into())])
                ),
            ]))
        );
        assert_eq!(serde_json::to_value(&value).expect("serializes"), json);
        assert_eq!(Value::from_json(&json), value);
    }

    #[test]
    fn text_serializes_as_a_plain_string() {
        // The derive this replaced could not even do this.
        assert_eq!(
            serde_json::to_string(&Value::Text("hi".into())).expect("serializes"),
            r#""hi""#
        );
        assert_eq!(
            serde_json::to_string(&Value::None).expect("serializes"),
            "null"
        );
    }

    #[test]
    fn structured_kinds_round_trip_with_a_discriminator() {
        let values = vec![
            Value::Prob(0.8),
            Value::Choice(Choice {
                item: Some(Box::new(Value::Record(BTreeMap::from([(
                    "p".to_string(),
                    Value::Prob(0.1),
                )])))),
                index: Some(0),
                ..choice()
            }),
            Value::Level(Level {
                level: 1,
                score: 1.2,
                normalized: 0.6,
                confidence: 0.7,
                probabilities: BTreeMap::from([("a".to_string(), 0.4), ("b".to_string(), 0.6)]),
                names: vec!["a".into(), "b".into(), "c".into()],
            }),
            Value::Handle(Handle {
                capability: "claude".into(),
                id: "dev-1".into(),
                fields: BTreeMap::from([("pid".to_string(), json!(42))]),
            }),
            Value::PauseResult(PauseResult {
                answer: "yes".into(),
                text: Some("go".into()),
            }),
            Value::List(vec![Value::Prob(0.2), Value::Text("x".into())]),
        ];
        for value in values {
            let json = serde_json::to_value(&value).expect("serializes");
            let back: Value = serde_json::from_value(json.clone()).expect("parses back");
            assert_eq!(back, value, "{json}");
        }
        let json = serde_json::to_value(Value::Prob(0.8)).expect("serializes");
        assert_eq!(json, json!({"$jev": "prob", "value": 0.8}));
        let json = serde_json::to_value(Value::Choice(choice())).expect("serializes");
        assert_eq!(json["$jev"], "choice");
        assert_eq!(json["label"], "stuck");
    }

    #[test]
    fn to_json_of_a_choice_is_plain() {
        // Spec 6.9: Jev sees `{label, confidence, probabilities}` and nothing
        // that hints at the runtime's own representation.
        let json = Value::Choice(choice()).to_json();
        assert_eq!(
            json,
            json!({"label": "stuck", "confidence": 0.8, "probabilities": {"stuck": 0.8, "other": 0.2}})
        );
        assert_eq!(Value::Prob(0.8).to_json(), json!(0.8));
        let handle = Value::Handle(Handle {
            capability: "claude".into(),
            id: "dev-1".into(),
            fields: BTreeMap::from([("pid".to_string(), json!(42))]),
        });
        assert_eq!(
            handle.to_json(),
            json!({"capability": "claude", "id": "dev-1", "pid": 42})
        );
    }

    #[test]
    fn an_unknown_discriminator_is_rejected() {
        let error = serde_json::from_value::<Value>(json!({"$jev": "wat"})).expect_err("rejects");
        assert!(error.to_string().contains("wat"));
    }

    #[test]
    fn a_type_error_names_both_types() {
        let error = Value::type_error("a list", &Value::Text("x".into()));
        assert_eq!(error.code, RuntimeErrorCode::TypeError);
        assert!(!error.retryable);
        assert_eq!(error.message, "expected a list, got text");
    }
}

// Trend extraction over the emergent, flexible per-category schema.
// We don't know a category's shape ahead of time, so we discover it:
//   - the "items" field   (the array-of-objects, e.g. exercises / blocks)
//   - the "label" field   (what names each item, e.g. name / task)
//   - the numeric metrics (weight / reps / sets / rpe / hours / ...)
// and emit a tidy long-format dataset the frontend pivots into charts.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use serde::Serialize;
use serde_json::{Map, Value};

const LABEL_PREFERENCES: &[&str] = &[
    "name", "task", "title", "label", "exercise", "activity", "item", "food", "meal", "type",
];

#[derive(Serialize)]
pub struct TrendRow {
    pub date: String,
    pub label: String,
    pub values: Map<String, Value>, // metric -> number
}

#[derive(Serialize)]
pub struct Trends {
    pub items_field: Option<String>,
    pub label_field: Option<String>,
    pub metrics: Vec<String>,
    pub labels: Vec<String>,
    pub rows: Vec<TrendRow>,
    pub count_by_date: Vec<(String, i64)>,
}

/// `entries` is (event_date, data_json-as-Value), already date-sorted.
pub fn build_trends(entries: &[(String, Value)]) -> Trends {
    let items_field = detect_items_field(entries);

    // Collect every item object once to detect label + metric fields.
    let all_items: Vec<&Map<String, Value>> = entries
        .iter()
        .flat_map(|(_, data)| items_of(data, &items_field))
        .collect();

    let label_field = detect_label_field(&all_items);

    let mut metric_set: BTreeSet<String> = BTreeSet::new();
    for it in &all_items {
        for (k, v) in it.iter() {
            if label_field.as_deref() != Some(k.as_str()) && v.is_number() {
                metric_set.insert(k.clone());
            }
        }
    }
    let metrics: Vec<String> = metric_set.into_iter().collect();

    // Build long-format rows + label order (by first appearance).
    let mut rows = Vec::new();
    let mut labels = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for (date, data) in entries {
        for it in items_of(data, &items_field) {
            let label = label_field
                .as_ref()
                .and_then(|lf| it.get(lf))
                .and_then(|v| v.as_str())
                .unwrap_or("—")
                .to_string();
            if seen.insert(label.clone()) {
                labels.push(label.clone());
            }
            let mut values = Map::new();
            for m in &metrics {
                if let Some(n) = it.get(m).filter(|v| v.is_number()) {
                    values.insert(m.clone(), n.clone());
                }
            }
            rows.push(TrendRow { date: date.clone(), label, values });
        }
    }

    let mut counts: BTreeMap<String, i64> = BTreeMap::new();
    for (date, _) in entries {
        *counts.entry(date.clone()).or_default() += 1;
    }

    Trends {
        items_field,
        label_field,
        metrics,
        labels,
        rows,
        count_by_date: counts.into_iter().collect(),
    }
}

/// The repeated-items field = the top-level key most often holding an array of
/// objects across entries (e.g. "exercises", "blocks").
fn detect_items_field(entries: &[(String, Value)]) -> Option<String> {
    let mut counts: HashMap<String, usize> = HashMap::new();
    for (_, data) in entries {
        if let Some(obj) = data.as_object() {
            for (k, v) in obj {
                if let Some(arr) = v.as_array() {
                    if arr.iter().any(|x| x.is_object()) {
                        *counts.entry(k.clone()).or_default() += 1;
                    }
                }
            }
        }
    }
    counts.into_iter().max_by_key(|(_, c)| *c).map(|(k, _)| k)
}

/// Item objects for one entry: the items_field array if present, else the data
/// object itself treated as a single item (flat categories like meals).
fn items_of<'a>(data: &'a Value, items_field: &Option<String>) -> Vec<&'a Map<String, Value>> {
    if let Some(f) = items_field {
        if let Some(arr) = data.get(f).and_then(|v| v.as_array()) {
            return arr.iter().filter_map(|x| x.as_object()).collect();
        }
    }
    data.as_object().map(|o| vec![o]).unwrap_or_default()
}

fn detect_label_field(items: &[&Map<String, Value>]) -> Option<String> {
    for pref in LABEL_PREFERENCES {
        if items.iter().any(|it| it.get(*pref).map_or(false, |v| v.is_string())) {
            return Some(pref.to_string());
        }
    }
    let mut counts: HashMap<String, usize> = HashMap::new();
    for it in items {
        for (k, v) in it.iter() {
            if v.is_string() {
                *counts.entry(k.clone()).or_default() += 1;
            }
        }
    }
    counts.into_iter().max_by_key(|(_, c)| *c).map(|(k, _)| k)
}

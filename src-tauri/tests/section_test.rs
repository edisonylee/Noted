// Deterministic unit tests for the section-header splitter (no model needed).
use tauri_app_lib::pipeline::{is_multi_topic_candidate, split_sections};

#[test]
fn splits_diary_into_tagged_sections() {
    let note = "6/2\n\n\
        Schedule:\n9-12 work on noted\n2-4 class\n\n\
        Gym —\nsquat 245x5x3\nbench 185x5\n\n\
        Food: oatmeal, chicken bowl, protein shake\n\n\
        felt a bit tired today";
    let segs = split_sections(note);
    let hints: Vec<Option<&str>> = segs.iter().map(|s| s.hint.as_deref()).collect();

    assert!(hints.contains(&Some("schedule")), "schedule routed: {hints:?}");
    assert!(hints.contains(&Some("gym")), "gym routed: {hints:?}");
    assert!(hints.contains(&Some("food")), "food routed: {hints:?}");
    // the leading "6/2" and the trailing prose are untagged segments
    assert!(hints.contains(&None), "untagged prose present: {hints:?}");

    // an inline body on the header line belongs to that section
    let food = segs.iter().find(|s| s.hint.as_deref() == Some("food")).unwrap();
    assert!(food.body.contains("oatmeal"), "food body: {}", food.body);

    // a header's following lines belong to it
    let gym = segs.iter().find(|s| s.hint.as_deref() == Some("gym")).unwrap();
    assert!(gym.body.contains("squat"), "gym body: {}", gym.body);
}

#[test]
fn no_headers_is_one_untagged_segment() {
    let segs = split_sections("just a normal brain dump about my day, no headers here");
    assert_eq!(segs.len(), 1);
    assert!(segs[0].hint.is_none());
}

#[test]
fn stoplist_words_are_not_headers() {
    // "Note:" / "TODO:" must not become categories
    let segs = split_sections("Note: remember to call mom\nTODO: buy milk");
    assert!(segs.iter().all(|s| s.hint.is_none()), "stoplisted labels stay untagged");
}

#[test]
fn long_or_sentence_heads_are_not_headers() {
    // a colon mid-sentence (head > 3 words) is not a routing header
    let segs = split_sections("i went to the store today: bought eggs and milk");
    assert_eq!(segs.len(), 1);
    assert!(segs[0].hint.is_none());
}

#[test]
fn markdown_and_trailing_dash_headers() {
    let segs = split_sections("## Work\nshipped the parser\n\nSchedule -\n9am standup");
    let hints: Vec<Option<&str>> = segs.iter().map(|s| s.hint.as_deref()).collect();
    assert!(hints.contains(&Some("work")), "markdown header: {hints:?}");
    assert!(hints.contains(&Some("schedule")), "trailing-dash header: {hints:?}");
}

#[test]
fn mid_line_dash_is_not_a_header() {
    // A prose body line with a mid-sentence em-dash must NOT be read as a header
    // and steal the section it sits under (regression: "Food:" + "chipotle bowl —
    // chicken..." used to route the food body to a bogus "chipotle bowl" category).
    let segs = split_sections("Food:\nchipotle bowl — chicken, rice, guac");
    let hints: Vec<Option<&str>> = segs.iter().map(|s| s.hint.as_deref()).collect();
    assert!(hints.contains(&Some("food")), "food header kept: {hints:?}");
    assert!(!hints.contains(&Some("chipotle bowl")), "mid-line dash is not a header: {hints:?}");
    let food = segs.iter().find(|s| s.hint.as_deref() == Some("food")).unwrap();
    assert!(food.body.contains("chipotle bowl"), "dash line stays in food body: {}", food.body);
}

#[test]
fn multi_topic_gate() {
    // Short scraps are extracted whole (no segmentation); a long multi-sentence
    // dump is a candidate for semantic segmentation.
    assert!(!is_multi_topic_candidate("felt tired today"));
    assert!(!is_multi_topic_candidate("had lunch with Jake at Chipotle, then hit the gym"));
    let dump = "Woke up at 8 after only five hours of sleep and felt groggy. \
        Took 7.5mg of creatine with my coffee before heading out. \
        Hit the gym for legs: squat 245 for five sets of five, felt strong despite the bad sleep. \
        Grabbed a chipotle bowl with Jake afterward and we talked about his startup. \
        Spent the afternoon coding the parser and shipped it around four.";
    assert!(is_multi_topic_candidate(dump), "a long multi-sentence dump should segment");
}

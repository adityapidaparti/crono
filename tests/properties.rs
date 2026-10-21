//! Property-based tests (Hegel) for crono's pure core.
//!
//! Each test name is a sentence about how crono behaves when you use it, and
//! the comment above it says why that matters for your food log. Hegel tries
//! many inputs per test; when one breaks the sentence it shrinks it to the
//! smallest example and prints it, e.g. `let typed = "--1";`.
//!
//! `cargo test --test properties` runs them (case counts are in `hegel.toml`);
//! `HEGEL_TEST_CASES=20000 cargo test --test properties` digs deeper.
//!
//! Not covered here, because the logic lives inline in `src/main.rs` or behind
//! the network: which entries `crono rm` deletes, `--pick`, `crono day` totals,
//! the account-timezone clock, and the HTTP/session layer.

use std::collections::{BTreeSet, HashMap};

use chrono::{Datelike, NaiveDate, Timelike};
use crono::client::{
    build_diary_export, estimate_kcal, find_measure, resolve_amount, Food, Measure,
    NutrientAmount, NUTRIENT_ENERGY_KCAL,
};
use crono::types::{cron_day, cron_time, parse_cron_day, Amount, DaySpec, Meal};
use hegel::extras::chrono as chrono_gs;
use hegel::extras::serde_json as json_gs;
use hegel::prelude::*;
use serde_json::{json, Map, Value};

// ---------------------------------------------------------------------------
// Shared generators
// ---------------------------------------------------------------------------

/// Dates the documented `YYYY-MM-DD` format can express: years 0000–9999.
fn any_date() -> chrono_gs::NaiveDateGenerator {
    chrono_gs::naive_dates()
        .min_value(NaiveDate::from_ymd_opt(0, 1, 1).unwrap())
        .max_value(NaiveDate::from_ymd_opt(9999, 12, 31).unwrap())
}

/// Every finite number a JSON response can carry.
fn finite_numbers() -> gs::FloatGenerator<f64> {
    gs::floats::<f64>().allow_nan(false).allow_infinity(false)
}

/// Quantities crono accepts after `-a`: positive and finite.
fn positive_amounts() -> gs::FloatGenerator<f64> {
    gs::floats::<f64>()
        .min_value_exclusive(0.0)
        .allow_nan(false)
        .allow_infinity(false)
}

/// Characters Rust's `trim` treats as whitespace.
const WHITESPACE: &[char] = &[' ', '\t', '\n', '\r', '\u{85}', '\u{a0}', '\u{2003}', '\u{3000}'];

/// Any run of whitespace, including none.
fn whitespace() -> impl PrintableGenerator<String> {
    gs::vecs(gs::sampled_from(WHITESPACE)).map(|cs: Vec<char>| cs.into_iter().collect::<String>())
}

const UNIT_WORDS: &[&str] = &[
    "g", "gram", "grams", "oz", "ounce", "ounces", "cup", "tbsp", "tsp", "serving", "large",
    "slice", "piece", "fl oz", "lb", "ml", "container",
];

/// Measure names that start with a number, as some Cronometer measures do.
const NUMBERED_MEASURE_NAMES: &[&str] = &["1 cup, chopped", "1/2 cup", "0.5 tbsp"];

/// A unit as typed or as a measure name: a common one, or arbitrary text.
#[hegel::composite]
fn unit_words(tc: &TestCase) -> String {
    tc.draw(hegel::one_of!(
        gs::sampled_from(UNIT_WORDS).map(|w: &str| w.to_string()),
        gs::sampled_from(NUMBERED_MEASURE_NAMES).map(|w: &str| w.to_string()),
        gs::text(),
    ))
}

/// A unit that can follow a number: it doesn't start with whitespace or with
/// something number-like (a digit, '.', ',' or '/'), which crono reads as
/// part of the number or rejects.
#[hegel::composite]
fn typed_units(tc: &TestCase) -> String {
    tc.draw(hegel::one_of!(
        gs::sampled_from(UNIT_WORDS).map(|w: &str| w.to_string()),
        gs::from_regex(r"[^0-9.,/\s].*"),
    ))
}

/// Lowercase words for measure-matching tests: common units or any letters.
#[hegel::composite]
fn lowercase_units(tc: &TestCase) -> String {
    tc.draw(hegel::one_of!(
        gs::sampled_from(UNIT_WORDS).map(|w: &str| w.to_string()),
        gs::from_regex(r"[a-z]+"),
    ))
}

/// Something shaped like an amount: a number (maybe a fraction like 3/4 or
/// 1 1/2, maybe malformed), spacing, maybe a unit.
#[hegel::composite]
fn typed_amounts(tc: &TestCase) -> String {
    let number = tc.draw(gs::from_regex(r"[0-9]*\.?[0-9]*( *[0-9]*\.?[0-9]*/[0-9]*\.?[0-9]*)?"));
    let spacing = tc.draw(whitespace());
    let unit = tc.draw(hegel::one_of!(gs::just(String::new()), typed_units()));
    format!("{number}{spacing}{unit}")
}

/// Something shaped like a day: a keyword, `-N` (any i64, so `--N` too),
/// `+N`, or an ISO date.
#[hegel::composite]
fn typed_days(tc: &TestCase) -> String {
    tc.draw(hegel::one_of!(
        gs::sampled_from(&["today", "yesterday", "tomorrow", " Yesterday ", "TODAY", ""][..])
            .map(|w: &str| w.to_string()),
        gs::integers::<i64>().map(|n| format!("-{n}")),
        gs::integers::<i64>().map(|n| format!("+{n}")),
        any_date().map(|d| d.format("%Y-%m-%d").to_string()),
    ))
}

const MEASURE_KINDS: &[&str] = &["Weight", "Volume", "Recipe", "Custom", ""];

/// Measure and food ids: usually small (so they collide with each other and
/// with defaults), sometimes anything.
fn ids() -> impl PrintableGenerator<i64> {
    hegel::one_of!(
        gs::integers::<i64>().min_value(0).max_value(5),
        gs::integers::<i64>(),
    )
}

/// A food as `get_food` returns it, with at least `min_measures` measures.
#[hegel::composite]
fn foods(tc: &TestCase, min_measures: usize) -> Food {
    // A measure id identifies one measure within its food, so ids are unique.
    let measure_ids: Vec<i64> = tc.draw(gs::vecs(ids()).min_size(min_measures).unique(true));
    let mut measures = Vec::new();
    for id in measure_ids.iter().copied() {
        let name = tc.draw(unit_words());
        let grams_per_unit = tc.draw(finite_numbers());
        let recipe_servings = tc.draw(finite_numbers());
        let kind = tc.draw(hegel::one_of!(
            gs::sampled_from(MEASURE_KINDS).map(|k: &str| k.to_string()),
            gs::text(),
        ));
        measures.push(Measure { id, name, value: grams_per_unit, amount: recipe_servings, kind });
    }
    let declares_existing_default = !measure_ids.is_empty() && tc.draw(gs::booleans());
    let default_measure_id = if declares_existing_default {
        tc.draw(gs::sampled_from(measure_ids))
    } else {
        tc.draw(ids())
    };
    // Protein, fat, carbs and others, so calorie lookups must find the right one.
    let mut nutrients: Vec<NutrientAmount> = tc
        .draw(gs::vecs(gs::sampled_from(&[203i64, 204, 205, 291, 307][..])))
        .into_iter()
        .map(|id| NutrientAmount { id, amount: 0.0 })
        .collect();
    for nutrient in &mut nutrients {
        nutrient.amount = tc.draw(finite_numbers());
    }
    // kcal per 100 g. No food tops ~900 (pure fat); the cap keeps labels
    // realistic, and overflow on huge quantities has its own test.
    if let Some(kcal) = tc.draw(gs::optional(gs::floats::<f64>().min_value(0.0).max_value(1_000_000.0))) {
        let at = tc.draw(gs::integers::<usize>().max_value(nutrients.len()));
        nutrients.insert(at, NutrientAmount { id: NUTRIENT_ENERGY_KCAL, amount: kcal });
    }
    Food {
        id: tc.draw(ids()),
        name: tc.draw(gs::text()),
        source: None,
        default_measure_id,
        measures,
        nutrients,
    }
}

fn measure(id: i64, name: &str) -> Measure {
    Measure { id, name: name.to_string(), value: 100.0, amount: 1.0, kind: "Weight".into() }
}

const WEIGHT_WORDS: &[&str] = &["g", "gram", "grams", "oz", "ounce", "ounces"];

/// Units crono reads as a literal weight rather than a measure name.
fn is_weight_word(unit: &str) -> bool {
    WEIGHT_WORDS.contains(&unit)
}

/// How many `unit`s a measure's name says it holds ("1/2 cup" holds half a
/// cup); 1 when the name doesn't start with a quantity of that unit. Pinned
/// on its own by the "named with a quantity" properties.
fn units_in_name(m: &Measure, unit: &str) -> f64 {
    match m.name.parse::<Amount>() {
        Ok(named) if named.unit.as_deref().is_some_and(|u| u.starts_with(unit)) => named.value,
        _ => 1.0,
    }
}

/// Whether a weight can be logged against one of the food's measures (any
/// measure but a Recipe serving).
fn can_be_weighed(food: &Food) -> bool {
    food.measures.iter().any(|m| m.kind != "Recipe")
}

fn food_with(measures: Vec<Measure>) -> Food {
    Food {
        id: 1,
        name: "test food".into(),
        source: None,
        default_measure_id: measures.first().map(|m| m.id).unwrap_or(0),
        measures,
        nutrients: vec![],
    }
}

/// Uppercase the ASCII letters of `word` where `caps` says so.
fn recapitalize(word: &str, caps: &[bool]) -> String {
    word.chars()
        .zip(caps)
        .map(|(c, &up)| if up { c.to_ascii_uppercase() } else { c })
        .collect()
}

/// `word` with each ASCII letter randomly upper- or lowercase.
fn draw_capitalization(tc: &TestCase, word: &str) -> String {
    let n = word.chars().count();
    let caps = tc.draw(gs::vecs(gs::booleans()).min_size(n).max_size(n));
    recapitalize(&word.to_ascii_lowercase(), &caps)
}

// ---------------------------------------------------------------------------
// Amounts: what you type after `-a`
// ---------------------------------------------------------------------------

/// Typing anything after `-a` gives an amount or an error message, never a crash.
#[hegel::test]
fn any_typed_amount_is_accepted_or_rejected_without_crashing(tc: TestCase) {
    let typed = tc.draw(hegel::one_of!(gs::text(), typed_amounts()));
    let _ = typed.parse::<Amount>();
}

/// crono only accepts positive, finite quantities; zero, negative, or
/// impossibly large ones are refused when you type them.
#[hegel::test]
fn accepted_amounts_are_positive_and_finite(tc: TestCase) {
    let typed = tc.draw(hegel::one_of!(gs::text(), typed_amounts()));
    if let Ok(amount) = typed.parse::<Amount>() {
        assert!(amount.value.is_finite() && amount.value > 0.0, "{typed:?} became {amount:?}");
    }
}

/// Zero ("0", "0.0g", "00 cup") and negative ("-5g") amounts are rejected.
/// The explicit case is the input that tripped the earlier proptest suite.
#[hegel::test]
#[hegel::explicit_test_case(number = "0".to_string(), unit = "A".to_string())]
fn zero_and_negative_amounts_are_rejected(tc: TestCase) {
    let number = tc.draw(hegel::one_of!(
        gs::from_regex(r"0+\.?0*|\.0+"),
        gs::from_regex(r"-[0-9]*\.?[0-9]*"),
    ));
    let unit = tc.draw(hegel::one_of!(gs::just(String::new()), typed_units()));
    let typed = format!("{number}{unit}");
    assert!(typed.parse::<Amount>().is_err(), "{typed:?} was accepted");
}

/// An amount must start with a number: "cup", "a few", "½ cup" are rejected
/// rather than guessed at.
#[hegel::test]
fn amounts_must_start_with_a_number(tc: TestCase) {
    let typed = format!("{}{}", tc.draw(whitespace()), tc.draw(typed_units()));
    assert!(typed.parse::<Amount>().is_err(), "{typed:?} was accepted");
}

/// A number with two decimal points ("1.2.3g") is rejected.
#[hegel::test]
fn numbers_with_two_decimal_points_are_rejected(tc: TestCase) {
    let number = tc.draw(gs::from_regex(r"[0-9]*\.[0-9]*\.[0-9.]*"));
    let unit = tc.draw(hegel::one_of!(gs::just(String::new()), typed_units()));
    let typed = format!("{number}{unit}");
    assert!(typed.parse::<Amount>().is_err(), "{typed:?} was accepted");
}

/// A comma decimal ("1,5 cup") is rejected with a hint to write 1.5, rather
/// than read as 1 of some ",5 cup" measure.
#[hegel::test]
fn comma_decimals_are_rejected(tc: TestCase) {
    let whole = tc.draw(gs::from_regex(r"[0-9]+"));
    let decimals = tc.draw(gs::from_regex(r"[0-9]+"));
    let spacing = tc.draw(whitespace());
    let unit = tc.draw(hegel::one_of!(gs::just(String::new()), typed_units()));
    let typed = format!("{whole},{decimals}{spacing}{unit}");
    assert!(typed.parse::<Amount>().is_err(), "{typed:?} was accepted");
}

/// Two numbers in a row ("2 3 cup", "2 .5 cup") are rejected rather than read
/// as 2 of a "3 cup" measure. Only a fraction may follow a number ("1 1/2").
#[hegel::test]
fn a_second_number_is_rejected_unless_it_is_a_fraction(tc: TestCase) {
    let first = tc.draw(gs::from_regex(r"[1-9][0-9]*(\.[0-9]+)?"));
    let gap = format!(" {}", tc.draw(whitespace()));
    let second = tc.draw(gs::from_regex(r"[0-9]*\.?[0-9]+"));
    let spacing = tc.draw(whitespace());
    let unit = tc.draw(hegel::one_of!(gs::just(String::new()), typed_units()));
    let typed = format!("{first}{gap}{second}{spacing}{unit}");
    assert!(typed.parse::<Amount>().is_err(), "{typed:?} was accepted");
}

/// The number you type is exactly the quantity crono uses.
#[hegel::test]
fn the_number_typed_is_the_quantity_used(tc: TestCase) {
    let number = tc.draw(gs::from_regex(r"[0-9]+(\.[0-9]*)?|\.[0-9]+"));
    let quantity: f64 = number.parse().unwrap();
    tc.assume(quantity > 0.0 && quantity.is_finite());
    let spacing = tc.draw(whitespace());
    let unit = tc.draw(typed_units());
    let amount: Amount = format!("{number}{spacing}{unit}").parse().unwrap();
    assert_eq!(amount.value, quantity);
}

/// Everything after a decimal number is the unit, trimmed and lowercased;
/// with nothing after it, there is no unit (the food's default measure).
#[hegel::test]
fn the_rest_of_an_amount_is_its_unit(tc: TestCase) {
    let number = tc.draw(gs::from_regex(r"[1-9][0-9]*(\.[0-9]*)?"));
    // 310+ digit numbers overflow a float and are rejected (checked above).
    tc.assume(number.parse::<f64>().unwrap().is_finite());
    let spacing = tc.draw(whitespace());
    let unit = tc.draw(hegel::one_of!(gs::just(String::new()), typed_units()));
    let amount: Amount = format!("{number}{spacing}{unit}").parse().unwrap();
    let expected = unit.trim().to_ascii_lowercase();
    assert_eq!(amount.unit, (!expected.is_empty()).then_some(expected));
}

/// "120g", "120 g" and "  120   g  " are the same amount: spacing never
/// changes what gets logged (or whether it's accepted).
/// The explicit case is a number too big for a float: both forms must be
/// rejected, even though their error messages quote different text.
#[hegel::test]
#[hegel::explicit_test_case(
    number = format!("1{}", "0".repeat(309)),
    unit = "g".to_string(),
    before = String::new(),
    gap = " ".to_string(),
    after = String::new()
)]
fn spacing_does_not_change_an_amount(tc: TestCase) {
    let number = tc.draw(gs::from_regex(r"[1-9][0-9]*(\.[0-9]+)?"));
    let unit = tc.draw(typed_units());
    let before = tc.draw(whitespace());
    let gap = tc.draw(whitespace());
    let after = tc.draw(whitespace());
    let tidy = format!("{number}{unit}").parse::<Amount>().ok();
    let spaced = format!("{before}{number}{gap}{unit}{after}").parse::<Amount>().ok();
    assert_eq!(tidy, spaced);
}

/// "120g" and "120G" are the same amount: capitalization of the unit's ASCII
/// letters never changes what gets logged (or whether it's accepted).
#[hegel::test]
fn capitalization_does_not_change_an_amount(tc: TestCase) {
    let number = tc.draw(gs::from_regex(r"[1-9][0-9]*(\.[0-9]+)?"));
    let unit = tc.draw(typed_units());
    let recased = draw_capitalization(&tc, &unit);
    assert_eq!(
        format!("{number}{unit}").parse::<Amount>().ok(),
        format!("{number}{recased}").parse::<Amount>().ok()
    );
}

/// An accepted amount, written out, reads back as the same amount, for any
/// positive quantity down to the tiniest float.
#[hegel::test]
fn an_amount_written_out_reads_back_the_same(tc: TestCase) {
    let quantity = tc.draw(positive_amounts());
    let unit = tc.draw(hegel::one_of!(gs::just(String::new()), typed_units()));
    let amount: Amount = format!("{quantity}{unit}").parse().unwrap();
    assert_eq!(amount.to_string().parse::<Amount>(), Ok(amount));
}

/// "3/4" means three quarters and "1 1/2" one and a half, with or without a
/// unit after them.
#[hegel::test]
fn fractions_mean_their_value(tc: TestCase) {
    let whole = tc.draw(gs::integers::<u32>());
    let top = tc.draw(gs::integers::<u32>().min_value(1));
    let bottom = tc.draw(gs::integers::<u32>().min_value(1));
    let spacing = tc.draw(whitespace());
    let unit = tc.draw(hegel::one_of!(gs::just(String::new()), typed_units()));
    let (number, expected) = if tc.draw(gs::booleans()) {
        (format!("{whole} {top}/{bottom}"), whole as f64 + top as f64 / bottom as f64)
    } else {
        (format!("{top}/{bottom}"), top as f64 / bottom as f64)
    };
    let typed = format!("{number}{spacing}{unit}");
    assert_eq!(typed.parse::<Amount>().map(|a| a.value), Ok(expected), "{typed:?}");
}

/// A fraction with a zero denominator ("1/0 cup") is rejected.
#[hegel::test]
fn a_zero_denominator_is_rejected(tc: TestCase) {
    let top = tc.draw(gs::from_regex(r"[0-9]+"));
    let unit = tc.draw(hegel::one_of!(gs::just(String::new()), typed_units()));
    let typed = format!("{top}/0{unit}");
    assert!(typed.parse::<Amount>().is_err(), "{typed:?} was accepted");
}

/// Typing a fraction ("3/4 cup", "1 1/2 cup") logs exactly that many cups,
/// whichever cup measure the food has: "cup", "1 cup", or only one named
/// after some fraction ("1/2 cup"). Each of the nine float steps on the way
/// can be off by half an ulp, hence the 8-epsilon tolerance.
#[hegel::test]
fn fractions_log_exactly_that_many_cups(tc: TestCase) {
    let whole = tc.draw(gs::integers::<u32>().min_value(1));
    let top = tc.draw(gs::integers::<u32>().min_value(1));
    let bottom = tc.draw(gs::integers::<u32>().min_value(1));
    let (named_top, named_bottom) =
        (tc.draw(gs::integers::<u32>().min_value(1)), tc.draw(gs::integers::<u32>().min_value(1)));
    let named_cups = named_top as f64 / named_bottom as f64;
    let fraction_cup = Measure { value: 240.0 * named_cups, ..measure(2, &format!("{named_top}/{named_bottom} cup")) };
    let measures = match tc.draw(gs::integers::<u8>().max_value(3)) {
        0 => vec![Measure { value: 240.0, ..measure(1, "cup") }],
        1 => vec![Measure { value: 240.0, ..measure(1, "1 cup") }],
        2 => vec![fraction_cup],
        _ => vec![fraction_cup, Measure { value: 240.0, ..measure(1, "cup") }],
    };
    let (typed, intended_cups) = if tc.draw(gs::booleans()) {
        (format!("{whole} {top}/{bottom} cup"), whole as f64 + top as f64 / bottom as f64)
    } else {
        (format!("{top}/{bottom} cup"), top as f64 / bottom as f64)
    };
    let amount: Amount = typed.parse().unwrap();
    let resolved = resolve_amount(&food_with(measures), &amount).unwrap();
    let logged_cups = resolved.grams / 240.0;
    assert!(
        (logged_cups - intended_cups).abs() <= 8.0 * f64::EPSILON * intended_cups,
        "{typed:?} logged {logged_cups} cups ({})",
        resolved.display
    );
}

/// A measure named with a quantity holds that many units: if a food's only
/// measure is "2 slices" (90 g), "3 slices" logs 135 g.
#[hegel::test]
fn a_measure_named_with_a_quantity_holds_that_many_units(tc: TestCase) {
    let word = tc.draw(gs::from_regex(r"[a-z]+"));
    tc.assume(!is_weight_word(&word));
    let held = tc.draw(gs::from_regex(r"[1-9][0-9]*(\.[0-9]+)?"));
    let typed = tc.draw(gs::from_regex(r"[1-9][0-9]*(\.[0-9]+)?"));
    let grams = tc.draw(gs::floats::<f64>().min_value(1.0).max_value(10_000.0));
    let (held_n, typed_n): (f64, f64) = (held.parse().unwrap(), typed.parse().unwrap());
    let expected = typed_n / held_n * grams;
    tc.assume(expected.is_finite());
    let food = food_with(vec![Measure { value: grams, ..measure(1, &format!("{held} {word}")) }]);
    let resolved = resolve_amount(&food, &format!("{typed} {word}").parse().unwrap()).unwrap();
    assert_eq!(resolved.grams, expected, "\"{typed} {word}\" against \"{held} {word}\" ({grams} g)");
}

/// A quantity that belongs to another word doesn't scale the measure: with
/// only "12 fl oz can", "-a 2 can" is two cans.
#[hegel::test]
fn a_quantity_for_another_word_does_not_scale_the_measure(tc: TestCase) {
    let word = tc.draw(gs::from_regex(r"[a-z]+"));
    tc.assume(!is_weight_word(&word));
    let other = tc.draw(gs::from_regex(r"[a-z]+( [a-z]+)*"));
    tc.assume(!other.starts_with(&word));
    let held = tc.draw(gs::from_regex(r"[1-9][0-9]*"));
    let count = tc.draw(gs::floats::<f64>().min_value(0.01).max_value(1_000.0));
    let food = food_with(vec![Measure { value: 355.0, ..measure(1, &format!("{held} {other} {word}")) }]);
    let resolved = resolve_amount(&food, &Amount::new(count, Some(&word))).unwrap();
    assert_eq!(resolved.grams, count * 355.0, "{count} {word} against \"{held} {other} {word}\"");
}

// ---------------------------------------------------------------------------
// Days: what you type after `-d` (and `crono day/export/rm`)
// ---------------------------------------------------------------------------

/// Typing anything as a day gives a day or an error message, never a crash.
#[hegel::test]
fn any_typed_day_is_accepted_or_rejected_without_crashing(tc: TestCase) {
    let typed = tc.draw(hegel::one_of!(gs::text(), typed_days()));
    let _ = typed.parse::<DaySpec>();
}

/// "-N" resolves to exactly N days before today, or, when that is before
/// the earliest date chrono can represent, to an out-of-range error. It never
/// crashes or quietly lands on some other day.
#[hegel::test]
fn minus_n_resolves_exactly_or_reports_out_of_range(tc: TestCase) {
    let n = tc.draw(gs::integers::<i64>().min_value(0));
    let today = tc.draw(any_date());
    let day: DaySpec = format!("-{n}").parse().unwrap();
    let target = today.num_days_from_ce() as i128 - n as i128;
    let earliest = NaiveDate::MIN.num_days_from_ce() as i128;
    match day.resolve(today) {
        Ok(date) => assert_eq!(date.num_days_from_ce() as i128, target, "-{n} on {today} gave {date}"),
        Err(error) => assert!(target < earliest, "-{n} on {today} refused: {error}"),
    }
}

/// After the minus, a day offset is digits only: "-+1", "--1", "- 1" and
/// "-1x" are rejected rather than read as some other day.
#[hegel::test]
fn a_day_offset_is_a_minus_and_digits_only(tc: TestCase) {
    let before = tc.draw(gs::from_regex(r"[+\- a-z]*"));
    let digits = tc.draw(gs::from_regex(r"[0-9]+"));
    let after = tc.draw(gs::from_regex(r"[+a-z]*"));
    tc.assume(!(before.is_empty() && after.is_empty()));
    let typed = format!("-{before}{digits}{after}");
    assert!(typed.parse::<DaySpec>().is_err(), "{typed:?} was accepted");
}

/// "-N" means exactly N days before today.
#[hegel::test]
fn minus_n_means_exactly_n_days_ago(tc: TestCase) {
    let today = tc.draw(any_date());
    let target = tc.draw(any_date().max_value(today));
    let typed = format!("-{}", (today - target).num_days());
    assert_eq!(typed.parse::<DaySpec>().and_then(|d| d.resolve(today)), Ok(target));
}

/// A day typed with a leading minus never points forward in time, so a stray
/// extra dash can't file food under a day that hasn't happened.
#[hegel::test]
fn a_leading_minus_never_means_the_future(tc: TestCase) {
    let n = tc.draw(gs::integers::<i64>());
    let typed = format!("-{n}");
    if let Ok(DaySpec::Offset(days)) = typed.parse::<DaySpec>() {
        assert!(days <= 0, "{typed:?} means {days} days from today");
    }
}

/// "today", "yesterday" and "tomorrow" (any capitalization, surrounding
/// whitespace) mean today, the day before, and the day after.
#[hegel::test]
fn yesterday_today_and_tomorrow_mean_what_they_say(tc: TestCase) {
    let (word, offset) = tc.draw(gs::sampled_from(
        &[("today", 0i64), ("yesterday", -1), ("tomorrow", 1)][..],
    ));
    let typed = format!("{}{}{}", tc.draw(whitespace()), draw_capitalization(&tc, word), tc.draw(whitespace()));
    let today = tc.draw(any_date());
    let expected = today + chrono::Duration::days(offset);
    assert_eq!(typed.parse::<DaySpec>().and_then(|d| d.resolve(today)), Ok(expected));
}

/// Typing a calendar date as YYYY-MM-DD selects exactly that date, whatever today is.
#[hegel::test]
fn typing_a_date_selects_that_date(tc: TestCase) {
    let date = tc.draw(any_date());
    let today = tc.draw(any_date());
    let typed = date.format("%Y-%m-%d").to_string();
    assert_eq!(typed.parse::<DaySpec>().and_then(|d| d.resolve(today)), Ok(date));
}

/// A date typed without zero padding (2026-8-5, the form Cronometer's API
/// uses) selects the same date as the padded form.
#[hegel::test]
fn dates_typed_without_zero_padding_select_the_same_date(tc: TestCase) {
    let date = tc.draw(any_date());
    let today = tc.draw(any_date());
    let typed = format!("{:04}-{}-{}", date.year(), date.month(), date.day());
    assert_eq!(typed.parse::<DaySpec>().and_then(|d| d.resolve(today)), Ok(date));
}

/// Impossible dates (month 13, Feb 30, day 0) are rejected, never shifted to
/// a nearby real date.
#[hegel::test]
fn impossible_dates_are_rejected(tc: TestCase) {
    let year = tc.draw(gs::integers::<i32>().min_value(0).max_value(9999));
    let month = tc.draw(gs::integers::<u32>());
    let day = tc.draw(gs::integers::<u32>());
    let typed = format!("{year:04}-{month:02}-{day:02}");
    let expected = NaiveDate::from_ymd_opt(year, month, day).map(DaySpec::Date);
    assert_eq!(typed.parse::<DaySpec>().ok(), expected, "{typed:?}");
}

// ---------------------------------------------------------------------------
// Cronometer's wire format
// ---------------------------------------------------------------------------

/// Dates sent to Cronometer read year-month-day with no leading zeros
/// (2026-8-5), as the mobile API expects.
#[hegel::test]
fn dates_sent_to_cronometer_have_no_leading_zeros(tc: TestCase) {
    let date = tc.draw(any_date());
    let expected = [date.year().to_string(), date.month().to_string(), date.day().to_string()];
    assert_eq!(cron_day(date).split('-').collect::<Vec<_>>(), expected);
}

/// crono reads back every date it sends.
#[hegel::test]
fn crono_reads_back_every_date_it_sends(tc: TestCase) {
    let date = tc.draw(any_date());
    assert_eq!(parse_cron_day(&cron_day(date)), Some(date));
}

/// Response dates are read as exactly the year, month and day written, padded
/// or not; impossible ones (month 13, Feb 30) are rejected, never shifted to
/// a nearby real date.
#[hegel::test]
fn response_dates_are_read_exactly_or_rejected(tc: TestCase) {
    let year = tc.draw(gs::integers::<i32>().min_value(0).max_value(9999));
    let month = tc.draw(gs::integers::<u32>());
    let day = tc.draw(gs::integers::<u32>());
    let written = if tc.draw(gs::booleans()) {
        format!("{year:04}-{month:02}-{day:02}")
    } else {
        format!("{year}-{month}-{day}")
    };
    assert_eq!(parse_cron_day(&written), NaiveDate::from_ymd_opt(year, month, day));
}

/// Reading a date from any response text never crashes.
#[hegel::test]
fn reading_a_response_date_never_crashes(tc: TestCase) {
    let text = tc.draw(gs::text());
    let _ = parse_cron_day(&text);
}

/// Times sent to Cronometer are H:M:S with no leading zeros, and each field is
/// the actual hour, minute and second.
#[hegel::test]
fn times_sent_to_cronometer_are_unpadded_hour_minute_second(tc: TestCase) {
    let time = tc.draw(chrono_gs::naive_times());
    let expected = [time.hour(), time.minute(), time.second()].map(|n| n.to_string());
    assert_eq!(cron_time(time).split(':').collect::<Vec<_>>(), expected, "{time}");
}

// ---------------------------------------------------------------------------
// Meals: `-m` and how entries are filed
// ---------------------------------------------------------------------------

/// The spellings `crono log --help` documents, plus "snack" and "none", which
/// crono also accepts.
const MEAL_SPELLINGS: &[(Meal, &str)] = &[
    (Meal::Breakfast, "breakfast"),
    (Meal::Breakfast, "b"),
    (Meal::Lunch, "lunch"),
    (Meal::Lunch, "l"),
    (Meal::Dinner, "dinner"),
    (Meal::Dinner, "d"),
    (Meal::Snacks, "snacks"),
    (Meal::Snacks, "snack"),
    (Meal::Snacks, "s"),
    (Meal::Uncategorized, "uncategorized"),
    (Meal::Uncategorized, "none"),
    (Meal::Uncategorized, "u"),
];

/// Every meal name and abbreviation works with any capitalization and
/// surrounding whitespace.
#[hegel::test]
fn every_meal_spelling_works_in_any_capitalization(tc: TestCase) {
    let (meal, word) = tc.draw(gs::sampled_from(MEAL_SPELLINGS));
    let typed = format!("{}{}{}", tc.draw(whitespace()), draw_capitalization(&tc, word), tc.draw(whitespace()));
    assert_eq!(typed.parse::<Meal>(), Ok(meal));
}

/// Anything that isn't one of those spellings is an error, so a typo never
/// files food under the wrong meal.
#[hegel::test]
fn anything_else_is_not_a_meal(tc: TestCase) {
    let typed = tc.draw(hegel::one_of!(
        gs::text(),
        gs::sampled_from(MEAL_SPELLINGS).map(|(_, w): (Meal, &str)| w.to_string()),
        gs::from_regex(r"[bldsu][a-z]*"),
    ));
    let spelled = typed.trim().to_ascii_lowercase();
    let expected = MEAL_SPELLINGS.iter().find(|(_, w)| *w == spelled).map(|(m, _)| *m);
    assert_eq!(typed.parse::<Meal>().ok(), expected, "{typed:?}");
}

/// Only the diary-group field of an entry's `order` (bits 16–31) decides
/// which meal `crono day` files it under: 1–4 are breakfast, lunch, dinner,
/// snacks, and anything else is uncategorized.
#[hegel::test]
fn the_diary_group_alone_decides_the_meal_shown(tc: TestCase) {
    let high_bits = tc.draw(gs::integers::<i64>());
    let group = tc.draw(gs::integers::<u32>().max_value(0xFFFF));
    let low_bits = tc.draw(gs::integers::<u32>().max_value(0xFFFF));
    let order = (high_bits << 32) | ((group as i64) << 16) | low_bits as i64;
    let expected = match group {
        1 => Meal::Breakfast,
        2 => Meal::Lunch,
        3 => Meal::Dinner,
        4 => Meal::Snacks,
        _ => Meal::Uncategorized,
    };
    assert_eq!(Meal::from_order(order), expected);
}

/// Each meal is written as `(group << 16) | 1`, the layout the app uses, with
/// groups 0–4 for uncategorized, breakfast, lunch, dinner, snacks. Five meals,
/// so all are checked.
#[test]
fn every_meal_is_written_the_way_the_app_writes_it() {
    let app = [
        (Meal::Uncategorized, 0x0_0001),
        (Meal::Breakfast, 0x1_0001),
        (Meal::Lunch, 0x2_0001),
        (Meal::Dinner, 0x3_0001),
        (Meal::Snacks, 0x4_0001),
    ];
    for (meal, order) in app {
        assert_eq!(meal.pack_order(), order, "{meal}");
        assert_eq!(Meal::from_order(order), meal, "{order:#x}");
    }
}

/// Every meal name crono prints can be typed back in as `-m`.
#[test]
fn every_meal_name_crono_prints_can_be_typed_back() {
    for meal in Meal::ALL {
        assert_eq!(meal.to_string().parse::<Meal>(), Ok(meal));
    }
}

/// README: with no `--meal`, crono picks by hour: 04–09 breakfast, 10–13
/// lunch, 14–20 dinner, otherwise snacks. 24 hours, so all are checked.
#[test]
fn the_automatic_meal_follows_the_readme_schedule() {
    let readme = [
        (0..=3, Meal::Snacks),
        (4..=9, Meal::Breakfast),
        (10..=13, Meal::Lunch),
        (14..=20, Meal::Dinner),
        (21..=23, Meal::Snacks),
    ];
    for (hours, meal) in readme {
        for hour in hours {
            assert_eq!(Meal::from_hour(hour), meal, "{hour}:00");
        }
    }
}

// ---------------------------------------------------------------------------
// Logging: turning an amount into what gets sent
//
// `crono log` sends `resolve_amount`'s `measure_id` and `grams` to Cronometer
// unchanged (client.rs `add_serving`), so these check what Cronometer receives.
// ---------------------------------------------------------------------------

/// "N g" (or gram/grams) sends exactly N grams, whatever measures the food
/// has. (Foods measured only in recipe servings are refused; see below.)
#[hegel::test]
fn logging_in_grams_sends_exactly_that_many_grams(tc: TestCase) {
    let food = tc.draw(foods(1));
    tc.assume(can_be_weighed(&food));
    let grams = tc.draw(positive_amounts());
    let word = tc.draw(gs::sampled_from(&["g", "gram", "grams"][..]));
    let resolved = resolve_amount(&food, &Amount::new(grams, Some(word))).unwrap();
    assert_eq!(resolved.grams, grams);
}

/// "N oz" (or ounce/ounces) sends N × 28.349523125 grams, the international
/// avoirdupois ounce. (Amounts that overflow are the next test's job.)
#[hegel::test]
fn every_ounce_is_28_349523125_grams(tc: TestCase) {
    let food = tc.draw(foods(1));
    tc.assume(can_be_weighed(&food));
    let ounces = tc.draw(positive_amounts());
    tc.assume((ounces * 28.349523125).is_finite());
    let word = tc.draw(gs::sampled_from(&["oz", "ounce", "ounces"][..]));
    let resolved = resolve_amount(&food, &Amount::new(ounces, Some(word))).unwrap();
    assert_eq!(resolved.grams, ounces * 28.349523125);
}

/// Whatever crono logs, Cronometer receives a real number of grams; amounts
/// too large for that are refused. (JSON has no infinity, so it would go out
/// as `null`.)
#[hegel::test]
fn logged_grams_are_always_a_real_number(tc: TestCase) {
    let food = tc.draw(foods(1));
    let names: Vec<String> = food.measures.iter().map(|m| m.name.clone()).collect();
    let unit = tc.draw(hegel::one_of!(
        gs::just(None),
        gs::sampled_from(&["g", "grams", "oz", "ounces"][..]).map(|w: &str| Some(w.to_string())),
        gs::sampled_from(names).map(Some),
    ));
    let quantity = tc.draw(positive_amounts());
    if let Ok(resolved) = resolve_amount(&food, &Amount::new(quantity, unit.as_deref())) {
        assert!(resolved.grams.is_finite(), "{quantity} {unit:?} → {} g", resolved.grams);
    }
}

/// A food measured only in recipe servings can't be logged by weight; crono
/// says to log a serving count instead of guessing.
#[hegel::test]
fn recipe_only_foods_cannot_be_logged_by_weight(tc: TestCase) {
    let mut food = tc.draw(foods(1));
    for m in &mut food.measures {
        m.kind = "Recipe".into();
    }
    let quantity = tc.draw(positive_amounts());
    let word = tc.draw(gs::sampled_from(&["g", "gram", "grams", "oz", "ounce", "ounces"][..]));
    let result = resolve_amount(&food, &Amount::new(quantity, Some(word)));
    assert!(result.is_err(), "\"{quantity} {word}\" → {result:?}");
}

/// Logging by weight (g, oz) never points the entry at a recipe's serving
/// measure. Cronometer reads `grams` on a Recipe measure as servings, so
/// "100g" there would record 100 servings. Refusing is fine; mislabeling isn't.
#[hegel::test]
fn weighing_food_never_points_at_a_recipe_serving(tc: TestCase) {
    let food = tc.draw(foods(1));
    let quantity = tc.draw(positive_amounts());
    let word = tc.draw(gs::sampled_from(&["g", "gram", "grams", "oz", "ounce", "ounces"][..]));
    if let Ok(resolved) = resolve_amount(&food, &Amount::new(quantity, Some(word))) {
        let measure = food.measure(resolved.measure_id).unwrap();
        assert_ne!(measure.kind, "Recipe", "\"{quantity} {word}\" points at {measure:?}");
    }
}

/// A bare number ("-a 2") logs that many of the food's declared default
/// measure (or its first measure when the declared one is missing): for a
/// non-recipe measure, 2 × the measure's grams. It's refused only when that
/// total is too large to send. (Recipe serving math isn't pinned down: it
/// depends on how Cronometer reads recipe measures.)
#[hegel::test]
fn a_bare_number_logs_that_many_of_the_default_measure(tc: TestCase) {
    let food = tc.draw(foods(1));
    let count = tc.draw(positive_amounts());
    let default = food.measure(food.default_measure_id).unwrap_or(&food.measures[0]);
    let result = resolve_amount(&food, &Amount::new(count, None));
    if default.kind == "Recipe" {
        if let Ok(resolved) = result {
            assert_eq!(resolved.measure_id, default.id);
        }
        return;
    }
    let expected = count * default.value;
    if expected.is_finite() {
        let resolved = result.unwrap();
        assert_eq!(resolved.measure_id, default.id);
        assert_eq!(resolved.grams, expected);
    } else {
        assert!(result.is_err(), "{count} × {} g gave {result:?}", default.value);
    }
}

/// "N cup" logs the measure crono's lookup picks for "cup" (exact name, then
/// prefix, then substring) and sends N cups of it: N × its grams, or N × 2
/// for a measure named "1/2 cup". It's refused only when that total is too
/// large to send. (Recipe serving math isn't pinned down.)
#[hegel::test]
fn n_of_a_measure_sends_n_times_its_grams(tc: TestCase) {
    let food = tc.draw(foods(1));
    let names: Vec<String> = food.measures.iter().map(|m| m.name.clone()).collect();
    let name = tc.draw(gs::sampled_from(names));
    let count = tc.draw(positive_amounts());
    let amount = Amount::new(count, Some(&name));
    let unit = amount.unit.clone().unwrap();
    tc.assume(!is_weight_word(&unit));
    let chosen = find_measure(&food, &unit).expect("a measure's own name matches it");
    let result = resolve_amount(&food, &amount);
    if chosen.kind == "Recipe" {
        if let Ok(resolved) = result {
            assert_eq!(resolved.measure_id, chosen.id);
        }
        return;
    }
    let expected = count / units_in_name(chosen, &unit) * chosen.value;
    if expected.is_finite() {
        let resolved = result.unwrap();
        assert_eq!(resolved.measure_id, chosen.id, "{unit:?}");
        assert_eq!(resolved.grams, expected, "{count} {unit:?} of {chosen:?}");
    } else {
        assert!(result.is_err(), "{count} {unit:?} of {chosen:?} gave {result:?}");
    }
}

/// "-a 2 cup" logs the measure named exactly "cup", even when measures that
/// start with or contain "cup" are listed before it.
#[hegel::test]
fn a_typed_unit_logs_the_exactly_named_measure(tc: TestCase) {
    let unit = tc.draw(lowercase_units());
    tc.assume(!is_weight_word(&unit));
    let suffixes = tc.draw(gs::vecs(gs::from_regex(r"[a-z ,]+")).min_size(1));
    let prefixes = tc.draw(gs::vecs(gs::from_regex(r"[a-z ,]+")));
    let mut measures: Vec<Measure> = suffixes
        .iter()
        .map(|s| format!("{unit}{s}"))
        .chain(prefixes.iter().map(|p| format!("{p}{unit}")))
        .enumerate()
        .map(|(i, name)| measure(i as i64 + 1, &name))
        .collect();
    let at = tc.draw(gs::integers::<usize>().max_value(measures.len()));
    measures.insert(at, measure(0, &unit));
    let count = tc.draw(positive_amounts().max_value(1e300));
    let resolved = resolve_amount(&food_with(measures), &Amount::new(count, Some(&unit))).unwrap();
    assert_eq!(resolved.measure_id, 0, "{resolved:?}");
}

/// A unit that names a measure exactly gets that measure, even when other
/// measures start with or contain the same word ("cup" vs "cup, diced").
#[hegel::test]
fn an_exact_measure_name_beats_partial_matches(tc: TestCase) {
    let unit = tc.draw(lowercase_units());
    let suffixes = tc.draw(gs::vecs(gs::from_regex(r"[a-z ,]+")).min_size(1));
    let prefixes = tc.draw(gs::vecs(gs::from_regex(r"[a-z ,]+")));
    let mut measures: Vec<Measure> = suffixes
        .iter()
        .map(|s| format!("{unit}{s}"))
        .chain(prefixes.iter().map(|p| format!("{p}{unit}")))
        .enumerate()
        .map(|(i, name)| measure(i as i64 + 1, &name))
        .collect();
    let at = tc.draw(gs::integers::<usize>().max_value(measures.len()));
    measures.insert(at, measure(0, &unit));
    assert_eq!(find_measure(&food_with(measures), &unit).map(|m| m.id), Some(0));
}

/// With no exact match, a measure that starts with the unit beats one that
/// merely contains it ("tbsp" picks "tbsp, heaping" over "1/2 tbsp").
#[hegel::test]
fn a_prefix_match_beats_a_substring_match(tc: TestCase) {
    let unit = tc.draw(lowercase_units());
    let suffix = tc.draw(gs::from_regex(r"[a-z ,]+"));
    let befores = tc.draw(gs::vecs(gs::from_regex(r"[a-z ,]+")).min_size(1));
    let afters = tc.draw(gs::vecs(gs::from_regex(r"[a-z ,]*")).min_size(befores.len()));
    let containing: Vec<String> = befores
        .iter()
        .zip(&afters)
        .map(|(b, a)| format!("{b}{unit}{a}"))
        .filter(|name| !name.starts_with(&unit))
        .collect();
    tc.assume(!containing.is_empty());
    let mut measures: Vec<Measure> =
        containing.iter().enumerate().map(|(i, name)| measure(i as i64 + 1, name)).collect();
    let at = tc.draw(gs::integers::<usize>().max_value(measures.len()));
    measures.insert(at, measure(0, &format!("{unit}{suffix}")));
    assert_eq!(find_measure(&food_with(measures), &unit).map(|m| m.id), Some(0));
}

/// Capitalization doesn't matter when matching a unit to a measure: "CUP",
/// "Cup" and "cup" find the same measure.
#[hegel::test]
fn measure_matching_ignores_capitalization(tc: TestCase) {
    let food = tc.draw(foods(1));
    let names: Vec<String> = food.measures.iter().map(|m| m.name.clone()).collect();
    let name = tc.draw(gs::sampled_from(names));
    let recased = draw_capitalization(&tc, &name);
    let expected = find_measure(&food, &name.to_ascii_lowercase()).map(|m| m.id);
    assert!(expected.is_some(), "a measure's own name didn't match it");
    assert_eq!(find_measure(&food, &recased).map(|m| m.id), expected, "{recased:?}");
}

/// A unit that matches none of the food's measures is an error that lists
/// all the measures you can use. crono never silently logs some other measure.
#[hegel::test]
fn an_unknown_unit_is_an_error_listing_the_choices(tc: TestCase) {
    // Measure names use only a–f, digits, spaces and commas; the unit always
    // contains a letter from s–y, so it can't match one. Neither alphabet can
    // spell g/gram/grams/oz/ounce/ounces.
    let names = tc.draw(gs::vecs(gs::from_regex(r"[a-f0-9 ,]*")).min_size(1));
    let unit = tc.draw(gs::from_regex(r"[a-f]*[s-y][a-f]*"));
    let food = food_with(names.iter().enumerate().map(|(i, n)| measure(i as i64, n)).collect());
    let count = tc.draw(positive_amounts());
    let error = resolve_amount(&food, &Amount::new(count, Some(&unit)))
        .expect_err("an unknown unit resolved to a measure")
        .to_string();
    let choices = format!("available: {}", names.join(", "));
    assert!(error.ends_with(&choices), "{error:?} should end with {choices:?}");
}

/// A food with no measures can't be logged, whatever the amount.
#[hegel::test]
fn a_food_without_measures_cannot_be_logged(tc: TestCase) {
    let mut food = tc.draw(foods(0));
    food.measures.clear();
    let count = tc.draw(positive_amounts());
    let unit = tc.draw(gs::optional(unit_words()));
    assert!(resolve_amount(&food, &Amount::new(count, unit.as_deref())).is_err());
}

/// The measure an entry points at is always one of the food's own measures.
#[hegel::test]
fn logged_entries_point_at_one_of_the_foods_measures(tc: TestCase) {
    let food = tc.draw(foods(1));
    let names: Vec<String> = food.measures.iter().map(|m| m.name.clone()).collect();
    let unit = tc.draw(hegel::one_of!(
        gs::just(None),
        gs::sampled_from(&["g", "oz"][..]).map(|w: &str| Some(w.to_string())),
        gs::sampled_from(names).map(Some),
    ));
    let count = tc.draw(positive_amounts());
    if let Ok(resolved) = resolve_amount(&food, &Amount::new(count, unit.as_deref())) {
        assert!(food.measure(resolved.measure_id).is_some(), "{resolved:?}");
    }
}

/// `crono log` and `crono day` agree on how to read an entry: both treat its
/// `grams` as servings exactly when it points at a Recipe measure. If they
/// disagree, the kcal `log` prints won't match what `day` shows for the same
/// entry.
#[hegel::test]
fn entries_count_servings_exactly_when_they_point_at_a_recipe_measure(tc: TestCase) {
    let food = tc.draw(foods(1));
    let names: Vec<String> = food.measures.iter().map(|m| m.name.clone()).collect();
    let unit = tc.draw(hegel::one_of!(
        gs::just(None),
        gs::sampled_from(&["g", "grams", "oz", "ounces"][..]).map(|w: &str| Some(w.to_string())),
        gs::sampled_from(names).map(Some),
    ));
    let count = tc.draw(positive_amounts());
    if let Ok(resolved) = resolve_amount(&food, &Amount::new(count, unit.as_deref())) {
        let kind = &food.measure(resolved.measure_id).unwrap().kind;
        assert_eq!(resolved.is_recipe, kind == "Recipe", "unit {unit:?} → {resolved:?}, measure kind {kind:?}");
    }
}

/// For a food without Recipe measures (so its calorie label is per 100 g),
/// the calories crono estimates for N g are the label × N ÷ 100. The label
/// is planted among the food's other nutrients so the test knows its value.
#[hegel::test]
fn estimated_calories_are_the_per_100g_label_scaled_by_weight(tc: TestCase) {
    let mut food = tc.draw(foods(1));
    for m in &mut food.measures {
        if m.kind == "Recipe" {
            m.kind = "Weight".into();
        }
    }
    let label = tc.draw(gs::floats::<f64>().min_value(0.0).max_value(1_000_000.0));
    food.nutrients.retain(|n| n.id != NUTRIENT_ENERGY_KCAL);
    let at = tc.draw(gs::integers::<usize>().max_value(food.nutrients.len()));
    food.nutrients.insert(at, NutrientAmount { id: NUTRIENT_ENERGY_KCAL, amount: label });
    let grams = tc.draw(positive_amounts());
    let resolved = resolve_amount(&food, &Amount::new(grams, Some("g"))).unwrap();
    assert_eq!(estimate_kcal(&food, &resolved), Some(label * grams / 100.0), "{grams} g at {label} kcal/100 g");
}

/// When Cronometer has no calorie figure for a food, crono's estimate is
/// "unknown", never 0.
#[hegel::test]
fn missing_calorie_data_gives_no_estimate_rather_than_zero(tc: TestCase) {
    let mut food = tc.draw(foods(1));
    food.nutrients.retain(|n| n.id != NUTRIENT_ENERGY_KCAL);
    let count = tc.draw(positive_amounts());
    let Ok(resolved) = resolve_amount(&food, &Amount::new(count, None)) else { tc.reject() };
    assert_eq!(estimate_kcal(&food, &resolved), None);
}

/// Resolving any amount against any food gives a result or an error, never a crash.
#[hegel::test]
fn resolving_any_amount_never_crashes(tc: TestCase) {
    let food = tc.draw(foods(0));
    let value = tc.draw(gs::floats::<f64>());
    let unit = tc.draw(gs::optional(unit_words()));
    let _ = resolve_amount(&food, &Amount::new(value, unit.as_deref()));
}

// ---------------------------------------------------------------------------
// Export
// ---------------------------------------------------------------------------

const ENTRY_TYPES: &[&str] = &["Serving", "Exercise", "Note", "Biometric", "Fasting"];

/// Fields Cronometer might add to any object.
#[hegel::composite]
fn extra_fields(tc: &TestCase) -> Map<String, Value> {
    let fields: HashMap<String, Value> = tc.draw(gs::hashmaps(gs::text(), json_gs::values()));
    fields.into_iter().collect()
}

/// A diary entry: a known or unknown type, an integer `foodId` (the export
/// command rejects Servings without one before this point), and extra fields.
#[hegel::composite]
fn diary_entries(tc: &TestCase) -> Value {
    let mut entry = tc.draw(extra_fields());
    let kind = tc.draw(hegel::one_of!(
        gs::sampled_from(ENTRY_TYPES).map(|t: &str| t.to_string()),
        gs::text(),
    ));
    entry.insert("type".into(), json!(kind));
    entry.insert("foodId".into(), json!(tc.draw(ids())));
    Value::Object(entry)
}

/// A food record, with or without an integer `id`.
#[hegel::composite]
fn food_records(tc: &TestCase) -> Value {
    let mut record = tc.draw(extra_fields());
    if let Some(id) = tc.draw(gs::optional(ids())) {
        record.insert("id".into(), json!(id));
    }
    Value::Object(record)
}

#[hegel::composite]
fn diary_responses(tc: &TestCase) -> Value {
    let mut response = tc.draw(extra_fields());
    response.insert("diary".into(), Value::Array(tc.draw(gs::vecs(diary_entries()))));
    Value::Object(response)
}

#[hegel::composite]
fn foods_responses(tc: &TestCase) -> Value {
    let mut response = tc.draw(extra_fields());
    response.insert("foods".into(), Value::Array(tc.draw(gs::vecs(food_records()))));
    Value::Object(response)
}

/// Near-misses: the right key holding the wrong shape, or a list with some
/// non-object items.
#[hegel::composite]
fn broken_responses(tc: &TestCase, key: &'static str) -> Value {
    let records = tc.draw(hegel::one_of!(
        json_gs::values(),
        gs::vecs(hegel::one_of!(diary_entries(), json_gs::values())).map(Value::Array),
    ));
    json!({ key: records })
}

/// Export hands back Cronometer's diary and food responses unchanged,
/// including fields crono doesn't know about.
#[hegel::test]
fn export_keeps_cronometers_responses_unchanged(tc: TestCase) {
    let diary = tc.draw(diary_responses());
    let foods = tc.draw(foods_responses());
    let (day, user_id, timezone) = (tc.draw(any_date()), tc.draw(gs::integers::<i64>()), tc.draw(gs::text()));
    let export = build_diary_export(day, user_id, &timezone, diary.clone(), foods.clone()).unwrap();
    assert_eq!(export["diary_response"], diary);
    assert_eq!(export["foods_response"], foods);
}

/// `missing_food_ids` lists, once each, exactly the foods that a logged
/// serving uses but the food response doesn't describe. Exercises, notes and
/// other entries never count.
#[hegel::test]
fn export_flags_exactly_the_servings_with_missing_food_data(tc: TestCase) {
    let diary = tc.draw(diary_responses());
    let foods = tc.draw(foods_responses());
    let (day, user_id, timezone) = (tc.draw(any_date()), tc.draw(gs::integers::<i64>()), tc.draw(gs::text()));
    let export = build_diary_export(day, user_id, &timezone, diary.clone(), foods.clone()).unwrap();
    let listed: Vec<i64> =
        export["missing_food_ids"].as_array().unwrap().iter().map(|v| v.as_i64().unwrap()).collect();
    let flagged: BTreeSet<i64> = listed.iter().copied().collect();
    assert_eq!(flagged.len(), listed.len(), "an id is listed twice: {listed:?}");
    let described: BTreeSet<i64> =
        foods["foods"].as_array().unwrap().iter().filter_map(|f| f["id"].as_i64()).collect();
    let served: BTreeSet<i64> = diary["diary"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["type"] == "Serving")
        .map(|e| e["foodId"].as_i64().unwrap())
        .collect();
    for id in &flagged {
        assert!(served.contains(id), "flagged {id}, which no serving uses");
        assert!(!described.contains(id), "flagged {id}, which the food response describes");
    }
    for id in served.difference(&described) {
        assert!(flagged.contains(id), "serving of food {id} has no food data but isn't flagged");
    }
}

/// Export succeeds only when the diary response has a `diary` list and the
/// food response a `foods` list, each made of objects. Anything else is an
/// error, never a partial or empty export.
#[hegel::test]
fn export_refuses_malformed_responses(tc: TestCase) {
    let diary = tc.draw(hegel::one_of!(diary_responses(), broken_responses("diary"), json_gs::values()));
    let foods = tc.draw(hegel::one_of!(foods_responses(), broken_responses("foods"), json_gs::values()));
    let well_formed = |response: &Value, key: &str| {
        response.get(key).and_then(Value::as_array).is_some_and(|items| items.iter().all(Value::is_object))
    };
    let should_succeed = well_formed(&diary, "diary") && well_formed(&foods, "foods");
    let (day, user_id, timezone) = (tc.draw(any_date()), tc.draw(gs::integers::<i64>()), tc.draw(gs::text()));
    let result = build_diary_export(day, user_id, &timezone, diary, foods);
    assert_eq!(result.is_ok(), should_succeed, "{result:?}");
}

/// An export is labelled with the day and the account (user id, timezone) it
/// came from.
#[hegel::test]
fn export_is_labelled_with_its_day_and_account(tc: TestCase) {
    let diary = tc.draw(diary_responses());
    let foods = tc.draw(foods_responses());
    let (day, user_id, timezone) = (tc.draw(any_date()), tc.draw(gs::integers::<i64>()), tc.draw(gs::text()));
    let export = build_diary_export(day, user_id, &timezone, diary, foods).unwrap();
    assert_eq!(export["day"].as_str().map(str::parse::<NaiveDate>), Some(Ok(day)));
    assert_eq!(export["account"]["user_id"], json!(user_id));
    assert_eq!(export["account"]["timezone"], json!(timezone));
}

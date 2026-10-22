//! What `crono day` shows and which entries `crono rm` deletes, as pure
//! functions over the diary objects Cronometer returns.

use chrono::NaiveDate;
use serde_json::Value;
use std::collections::BTreeMap;

use crate::client::{Food, NUTRIENT_ENERGY_KCAL};
use crate::types::Meal;

fn is_food(entry: &Value) -> bool {
    entry.get("type").and_then(Value::as_str) == Some("Serving")
}

/// The diary objects `crono rm <id>` deletes: food entries whose `servingId`
/// is the id `crono day` shows. Exercises, notes and other entry types are
/// never deleted, even when their own `id` matches.
pub fn entries_to_remove(entries: Vec<Value>, serving_id: i64) -> Vec<Value> {
    entries
        .into_iter()
        .filter(|e| is_food(e) && e.get("servingId").and_then(Value::as_i64) == Some(serving_id))
        .collect()
}

/// A recipe entry's serving count: up to two decimals, "serving" for one.
fn serving_count(count: f64) -> String {
    let shown = format!("{count:.2}");
    let shown = shown.trim_end_matches('0').trim_end_matches('.');
    format!("{shown} serving{}", if shown == "1" { "" } else { "s" })
}

/// What `crono day` prints for a day's diary entries, given the foods they
/// use. A recipe entry's `grams` is a serving count, so it is shown as
/// servings. The total leaves out entries without a calorie estimate and says
/// how many it left out.
pub fn render_day(date: NaiveDate, entries: &[Value], foods: &[Food]) -> String {
    let servings: Vec<&Value> = entries.iter().filter(|e| is_food(e)).collect();
    if servings.is_empty() {
        return format!("{date}: no food logged\n");
    }
    let foods: BTreeMap<i64, &Food> = foods.iter().map(|f| (f.id, f)).collect();

    let mut by_meal: BTreeMap<u32, Vec<String>> = BTreeMap::new();
    let mut total_kcal = 0.0;
    let mut unestimated = 0;
    for e in &servings {
        let food_id = e.get("foodId").and_then(Value::as_i64).unwrap_or(0);
        let serving_id = e.get("servingId").and_then(Value::as_i64).unwrap_or(0);
        let grams = e.get("grams").and_then(Value::as_f64).unwrap_or(0.0);
        let order = e.get("order").and_then(Value::as_i64).unwrap_or(0);
        let meal = Meal::from_order(order);
        let (name, quantity, kcal) = match foods.get(&food_id) {
            Some(f) => {
                let measure_id = e.get("measureId").and_then(Value::as_i64).unwrap_or(0);
                let is_recipe = f.measure(measure_id).map(|m| m.kind == "Recipe").unwrap_or(false);
                let kcal = f
                    .nutrient(NUTRIENT_ENERGY_KCAL)
                    .map(|k| if is_recipe { k * grams } else { k * grams / 100.0 });
                let quantity = if is_recipe { serving_count(grams) } else { format!("{grams:.0} g") };
                (f.name.clone(), quantity, kcal)
            }
            None => (format!("food #{food_id}"), format!("{grams:.0} g"), None),
        };
        match kcal {
            Some(k) => total_kcal += k,
            None => unestimated += 1,
        }
        let kcal_str = kcal.map(|k| format!("{k:>6.0} kcal")).unwrap_or_else(|| "     ?".into());
        by_meal
            .entry(meal.group_number())
            .or_default()
            .push(format!("  [{serving_id}] {name} — {quantity}  {kcal_str}"));
    }

    let mut out = format!("{date}\n");
    for (group, lines) in by_meal {
        out += &format!("{}:\n", Meal::from_group_number(group));
        for line in lines {
            out += &format!("{line}\n");
        }
    }
    let left_out = match unestimated {
        0 => String::new(),
        1 => " (1 entry without calories)".into(),
        n => format!(" ({n} entries without calories)"),
    };
    out += &format!("total ≈ {total_kcal:.0} kcal{left_out}\n");
    out
}

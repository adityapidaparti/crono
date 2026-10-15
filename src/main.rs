use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use serde_json::Value;
use std::collections::BTreeMap;

use crono::client::{estimate_kcal, resolve_amount, Client, Food, NUTRIENT_CARBS, NUTRIENT_FAT, NUTRIENT_PROTEIN};
use crono::session::{self, Credentials};
use crono::types::{Amount, DaySpec, Meal};

#[derive(Parser)]
#[command(name = "crono", about = "Log food to Cronometer from the terminal", version)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Authenticate and store a session (prompts for password)
    Login {
        /// Cronometer account email
        email: String,
        /// Remember credentials on disk (0600) for automatic re-auth
        #[arg(long)]
        remember: bool,
    },
    /// Search the food database
    Search {
        query: Vec<String>,
        #[arg(short = 'n', long, default_value_t = 10)]
        limit: usize,
    },
    /// Show a food's measures and macros (per 100 g)
    Food { food_id: i64 },
    /// Log a food to the diary: crono log chicken thigh -a 5oz -m dinner -d yesterday
    Log {
        /// Food search query (or #<id> for an exact food id)
        query: Vec<String>,
        /// Amount, e.g. "120g", "5oz", "1.5 cup", "2" (default measure)
        #[arg(short, long, default_value = "1")]
        amount: Amount,
        /// breakfast|lunch|dinner|snacks|uncategorized (b|l|d|s|u); default: by time of day
        #[arg(short, long)]
        meal: Option<Meal>,
        /// today|yesterday|tomorrow|-N|YYYY-MM-DD
        #[arg(short, long, default_value = "today", allow_hyphen_values = true)]
        day: DaySpec,
        /// Pick the Nth search result instead of the 1st (1-based)
        #[arg(long, default_value_t = 1)]
        pick: usize,
    },
    /// Show the diary for a day
    Day {
        #[arg(default_value = "today", allow_hyphen_values = true)]
        day: DaySpec,
    },
    /// Export original diary and food/nutrient metadata as JSON; cached session only
    Export {
        #[arg(default_value = "today", allow_hyphen_values = true)]
        day: DaySpec,
    },
    /// Remove a diary entry by serving id (ids shown by `crono day`)
    Rm {
        entry_id: i64,
        #[arg(short, long, default_value = "today", allow_hyphen_values = true)]
        day: DaySpec,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let mut client = Client::new()?;
    match cli.command {
        Command::Login { email, remember } => cmd_login(&mut client, &email, remember),
        Command::Search { query, limit } => cmd_search(&mut client, &query.join(" "), limit),
        Command::Food { food_id } => cmd_food(&mut client, food_id),
        Command::Log { query, amount, meal, day, pick } => {
            cmd_log(&mut client, &query.join(" "), &amount, meal, day, pick)
        }
        Command::Day { day } => cmd_day(&mut client, day),
        Command::Export { day } => {
            let (today, _) = client.account_now();
            let snapshot = client.export_day(day.resolve(today))?;
            println!("{}", serde_json::to_string_pretty(&snapshot)?);
            Ok(())
        },
        Command::Rm { entry_id, day } => cmd_rm(&mut client, entry_id, day),
    }
}

fn cmd_login(client: &mut Client, email: &str, remember: bool) -> Result<()> {
    // CRONO_PASSWORD lets login run non-interactively (no TTY needed).
    let password = match std::env::var("CRONO_PASSWORD") {
        Ok(p) if !p.is_empty() => p,
        _ => rpassword::prompt_password("Cronometer password: ")?,
    };
    let state = client.login(email, &password)?;
    println!("logged in as user {} (timezone {})", state.user_id, state.timezone);
    if remember {
        let path = session::save_credentials(&Credentials {
            username: email.to_string(),
            password,
        })?;
        println!("credentials saved to {} (0600) for automatic re-auth", path.display());
    }
    Ok(())
}

fn cmd_search(client: &mut Client, query: &str, limit: usize) -> Result<()> {
    let results = client.search(query)?;
    if results.is_empty() {
        println!("no results for '{query}'");
        return Ok(());
    }
    for f in results.iter().take(limit) {
        println!("#{:<12} {}  [{}]  ({})", f.id, f.name, f.source, f.measure_display_name);
    }
    Ok(())
}

fn cmd_food(client: &mut Client, food_id: i64) -> Result<()> {
    let food = client.get_food(food_id)?;
    println!("#{} {}", food.id, food.name);
    if let (Some(kcal), p, f, c) = (
        food.nutrient(crono::client::NUTRIENT_ENERGY_KCAL),
        food.nutrient(NUTRIENT_PROTEIN),
        food.nutrient(NUTRIENT_FAT),
        food.nutrient(NUTRIENT_CARBS),
    ) {
        println!(
            "per 100 g: {:.0} kcal, {:.1} P / {:.1} F / {:.1} C",
            kcal,
            p.unwrap_or(0.0),
            f.unwrap_or(0.0),
            c.unwrap_or(0.0)
        );
    }
    println!("measures:");
    for m in &food.measures {
        let default = if m.id == food.default_measure_id { "  (default)" } else { "" };
        println!("  {} — {:.1} g{}", m.name, m.value, default);
    }
    Ok(())
}

fn cmd_log(
    client: &mut Client,
    query: &str,
    amount: &Amount,
    meal: Option<Meal>,
    day: DaySpec,
    pick: usize,
) -> Result<()> {
    let food = if let Some(id) = query.strip_prefix('#').and_then(|s| s.parse::<i64>().ok()) {
        client.get_food(id)?
    } else {
        let results = client.search(query)?;
        let hit = results
            .get(pick.saturating_sub(1))
            .with_context(|| format!("no search result #{pick} for '{query}'"))?;
        client.get_food(hit.id)?
    };

    let resolved = resolve_amount(&food, amount)?;
    let (today, now) = client.account_now();
    let date = day.resolve(today);
    let meal = meal.unwrap_or_else(|| Meal::from_hour(chrono::Timelike::hour(&now)));

    client.add_serving(food.id, &resolved, meal, date, now)?;
    let kcal = estimate_kcal(&food, &resolved)
        .map(|k| format!(" ≈ {k:.0} kcal"))
        .unwrap_or_default();
    println!("logged: {} — {} → {} on {}{}", food.name, resolved.display, meal, date, kcal);
    Ok(())
}

fn cmd_day(client: &mut Client, day: DaySpec) -> Result<()> {
    let (today, _) = client.account_now();
    let date = day.resolve(today);
    let entries = client.get_diary(date)?;
    let servings: Vec<&Value> = entries
        .iter()
        .filter(|e| e.get("type").and_then(Value::as_str) == Some("Serving"))
        .collect();
    if servings.is_empty() {
        println!("{date}: no food logged");
        return Ok(());
    }

    let food_ids: Vec<i64> = servings
        .iter()
        .filter_map(|e| e.get("foodId").and_then(Value::as_i64))
        .collect();
    let foods: BTreeMap<i64, Food> = client
        .get_foods(&food_ids)?
        .into_iter()
        .map(|f| (f.id, f))
        .collect();

    let mut by_meal: BTreeMap<u32, Vec<String>> = BTreeMap::new();
    let mut total_kcal = 0.0;
    for e in &servings {
        let food_id = e.get("foodId").and_then(Value::as_i64).unwrap_or(0);
        let serving_id = e.get("servingId").and_then(Value::as_i64).unwrap_or(0);
        let grams = e.get("grams").and_then(Value::as_f64).unwrap_or(0.0);
        let order = e.get("order").and_then(Value::as_i64).unwrap_or(0);
        let meal = Meal::from_order(order);
        let (name, kcal) = match foods.get(&food_id) {
            Some(f) => {
                let measure_id = e.get("measureId").and_then(Value::as_i64).unwrap_or(0);
                let is_recipe = f.measure(measure_id).map(|m| m.kind == "Recipe").unwrap_or(false);
                let kcal = f
                    .nutrient(crono::client::NUTRIENT_ENERGY_KCAL)
                    .map(|k| if is_recipe { k * grams } else { k * grams / 100.0 });
                (f.name.clone(), kcal)
            }
            None => (format!("food #{food_id}"), None),
        };
        total_kcal += kcal.unwrap_or(0.0);
        let kcal_str = kcal.map(|k| format!("{k:>6.0} kcal")).unwrap_or_else(|| "     ?".into());
        by_meal
            .entry(meal.group_number())
            .or_default()
            .push(format!("  [{serving_id}] {name} — {grams:.0} g  {kcal_str}"));
    }

    println!("{date}");
    for (group, lines) in by_meal {
        println!("{}:", Meal::from_group_number(group));
        for line in lines {
            println!("{line}");
        }
    }
    println!("total ≈ {total_kcal:.0} kcal");
    Ok(())
}

fn cmd_rm(client: &mut Client, entry_id: i64, day: DaySpec) -> Result<()> {
    let (today, _) = client.account_now();
    let date = day.resolve(today);
    let entries = client.get_diary(date)?;
    let target: Vec<Value> = entries
        .into_iter()
        .filter(|e| {
            e.get("servingId").and_then(Value::as_i64) == Some(entry_id)
                || e.get("id").and_then(Value::as_i64) == Some(entry_id)
        })
        .collect();
    if target.is_empty() {
        bail!("no entry {entry_id} on {date} (see `crono day {date}` for ids)");
    }
    client.delete_entries(&target)?;
    println!("removed entry {entry_id} from {date}");
    Ok(())
}

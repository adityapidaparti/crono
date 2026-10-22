use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use serde_json::Value;

use crono::client::{estimate_kcal, resolve_amount, Client, NUTRIENT_CARBS, NUTRIENT_FAT, NUTRIENT_PROTEIN};
use crono::session::{self, Credentials};
use crono::types::{Amount, DaySpec, Meal, Pick};

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
        /// Amount, e.g. "120g", "5oz", "1.5 cup", "3/4 cup", "2" (default measure)
        #[arg(short, long, default_value = "1")]
        amount: Amount,
        /// breakfast|lunch|dinner|snacks|uncategorized (b|l|d|s|u); default: by time of day
        #[arg(short, long)]
        meal: Option<Meal>,
        /// today|yesterday|tomorrow|-N|YYYY-MM-DD
        #[arg(short, long, default_value = "today", allow_hyphen_values = true)]
        day: DaySpec,
        /// Pick the Nth search result instead of the 1st (1-based)
        #[arg(long, default_value = "1")]
        pick: Pick,
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
    /// Print the agent skill for this version, or install it with `skill install`
    Skill {
        #[command(subcommand)]
        action: Option<SkillCmd>,
    },
}

#[derive(Subcommand)]
enum SkillCmd {
    /// Write the skill to DIR/NAME/SKILL.md [default DIR: ~/.claude/skills, read by Claude Code and Grok CLI]
    Install {
        /// Skills directory of another agent
        #[arg(long)]
        dir: Option<std::path::PathBuf>,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    // The skill needs no session, so it runs before one is loaded.
    let command = match cli.command {
        Command::Skill { action } => return skill_cmd(action),
        command => command,
    };
    let mut client = Client::new()?;
    match command {
        Command::Login { email, remember } => cmd_login(&mut client, &email, remember),
        Command::Search { query, limit } => cmd_search(&mut client, &query.join(" "), limit),
        Command::Food { food_id } => cmd_food(&mut client, food_id),
        Command::Log { query, amount, meal, day, pick } => {
            cmd_log(&mut client, &query.join(" "), &amount, meal, day, pick)
        }
        Command::Day { day } => cmd_day(&mut client, day),
        Command::Export { day } => {
            let (today, _) = client.account_now()?;
            let snapshot = client.export_day(day.resolve(today).map_err(anyhow::Error::msg)?)?;
            println!("{}", serde_json::to_string_pretty(&snapshot)?);
            Ok(())
        },
        Command::Skill { .. } => unreachable!("handled before the session loads"),
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
    println!("logged in as user {} (timezone {})", state.user_id, state.timezone.as_deref().unwrap_or("unknown"));
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
    pick: Pick,
) -> Result<()> {
    let food = if let Some(id) = query.strip_prefix('#').and_then(|s| s.parse::<i64>().ok()) {
        client.get_food(id)?
    } else {
        let results = client.search(query)?;
        let hit = results
            .get(pick.index())
            .with_context(|| format!("no search result #{pick} for '{query}'"))?;
        client.get_food(hit.id)?
    };

    let resolved = resolve_amount(&food, amount)?;
    let (today, now) = client.account_now()?;
    let date = day.resolve(today).map_err(anyhow::Error::msg)?;
    let meal = meal.unwrap_or_else(|| Meal::from_hour(chrono::Timelike::hour(&now)));

    client.add_serving(food.id, &resolved, meal, date, now)?;
    let kcal = estimate_kcal(&food, &resolved)
        .map(|k| format!(" ≈ {k:.0} kcal"))
        .unwrap_or_default();
    println!("logged: {} — {} → {} on {}{}", food.name, resolved.display, meal, date, kcal);
    Ok(())
}

fn cmd_day(client: &mut Client, day: DaySpec) -> Result<()> {
    let (today, _) = client.account_now()?;
    let date = day.resolve(today).map_err(anyhow::Error::msg)?;
    let entries = client.get_diary(date)?;
    let food_ids: Vec<i64> = entries
        .iter()
        .filter(|e| e.get("type").and_then(Value::as_str) == Some("Serving"))
        .filter_map(|e| e.get("foodId").and_then(Value::as_i64))
        .collect();
    let foods = client.get_foods(&food_ids)?;
    print!("{}", crono::diary::render_day(date, &entries, &foods));
    Ok(())
}

fn cmd_rm(client: &mut Client, entry_id: i64, day: DaySpec) -> Result<()> {
    let (today, _) = client.account_now()?;
    let date = day.resolve(today).map_err(anyhow::Error::msg)?;
    let entries = client.get_diary(date)?;
    let target = crono::diary::entries_to_remove(entries, entry_id);
    if target.is_empty() {
        bail!("no entry {entry_id} on {date} (see `crono day {date}` for ids)");
    }
    client.delete_entries(&target)?;
    println!("removed entry {entry_id} from {date}");
    Ok(())
}

fn skill_cmd(action: Option<SkillCmd>) -> Result<()> {
    use crono::skill;
    match action {
        None => print!("{}", skill::SKILL),
        Some(SkillCmd::Install { dir }) => {
            let dir = match dir {
                Some(d) => d,
                None => skill::default_dir()?,
            };
            println!("{}", skill::install(&dir, skill::NAME, skill::SKILL)?);
        }
    }
    Ok(())
}

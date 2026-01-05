use std::env;

use rusqlite::{Connection, OptionalExtension};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = env::args().collect();
    let name = args.get(1).map(|s| s.to_lowercase());
    let conn = Connection::open("pokedex.db")?;

    if let Some(name) = name {
        let json: Option<String> = conn
            .query_row(
                "SELECT pokedex FROM trainers WHERE name = ?1",
                [&name],
                |row| row.get(0),
            )
            .optional()?;
        match json {
            Some(text) => {
                let list: Vec<String> = serde_json::from_str(&text)?;
                println!("trainer: {name}");
                println!("count: {}", list.len());
                for entry in list {
                    println!("- {}", entry);
                }
            }
            None => {
                println!("trainer not found: {name}");
            }
        }
        return Ok(());
    }

    let mut stmt = conn.prepare("SELECT name, pokedex FROM trainers ORDER BY name")?;
    let rows = stmt.query_map([], |row| {
        let name: String = row.get(0)?;
        let json: String = row.get(1)?;
        Ok((name, json))
    })?;
    for row in rows {
        let (name, json) = row?;
        let list: Vec<String> = serde_json::from_str(&json)?;
        println!("{name}: {} caught", list.len());
    }

    Ok(())
}

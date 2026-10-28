//! `sreg whoami`.

use sreg_core::models::{Grant, Kind};

use crate::cli::GlobalArgs;
use crate::context;
use crate::output::{indent, table, Output};

pub async fn run(global: &GlobalArgs, out: &Output) -> anyhow::Result<()> {
    let client = context::client(global).await?;
    let whoami = client.whoami().await?;

    out.emit(&whoami, || {
        println!("Project: {}", whoami.project);
        let roles: Vec<&str> = whoami.token_roles().iter().map(|r| r.as_str()).collect();
        println!("Roles:   {}", roles.join(", "));
        let sections = whoami.role_grants();
        if sections.is_empty() {
            let note = if whoami.token_roles().contains(&Kind::Admin) {
                "none (admin credentials have no grants)"
            } else {
                "none"
            };
            println!("Grants:  {note}");
        }
        // A token with both roles shows one list of grants per role.
        let many = sections.len() > 1;
        for (role, grants) in sections {
            let label = match (many, role) {
                (false, _) => "Grants".to_owned(),
                (true, role) => format!("{} grants", capitalize(role.as_str())),
            };
            if grants.is_empty() {
                println!("{label}: none");
            } else {
                println!("{label}:");
                // `transitive` only matters for read grants (docs/api.md 5.1).
                let transitive = role == Kind::Read;
                println!("{}", indent(&grants_table(grants, transitive), 2));
            }
        }
    })
}

fn capitalize(word: &str) -> String {
    let mut chars = word.chars();
    chars
        .next()
        .map(|first| first.to_uppercase().chain(chars).collect())
        .unwrap_or_default()
}

/// Formats grants as a table; with `transitive`, a column tells whether each grant also
/// opens the dependencies of its packages (the registry's default when it sent nothing).
pub fn grants_table(grants: &[Grant], transitive: bool) -> String {
    let rows: Vec<Vec<String>> = grants
        .iter()
        .map(|grant| {
            let mut row = vec![
                grant.eco.to_string(),
                grant.package.clone(),
                grant.versions.clone(),
            ];
            if transitive {
                let opens = grant.transitive.unwrap_or(true);
                row.push(if opens { "yes" } else { "no" }.to_owned());
            }
            row
        })
        .collect();
    if transitive {
        table(&["ECO", "PACKAGE", "VERSIONS", "TRANSITIVE"], &rows)
    } else {
        table(&["ECO", "PACKAGE", "VERSIONS"], &rows)
    }
}

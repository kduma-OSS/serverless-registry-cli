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
        println!("Kind:    {}", whoami.kind);
        if whoami.grants.is_empty() {
            let note = if whoami.kind == Kind::Admin {
                "none (admin credentials have no grants)"
            } else {
                "none"
            };
            println!("Grants:  {note}");
        } else {
            println!("Grants:");
            // `transitive` only matters for read credentials (docs/api.md 5.1).
            let transitive = whoami.kind == Kind::Read;
            println!("{}", indent(&grants_table(&whoami.grants, transitive), 2));
        }
    })
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

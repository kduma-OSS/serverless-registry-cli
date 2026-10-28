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
            println!("{}", indent(&grants_table(&whoami.grants), 2));
        }
    })
}

/// Formats grants as a table.
pub fn grants_table(grants: &[Grant]) -> String {
    let rows: Vec<Vec<String>> = grants
        .iter()
        .map(|grant| {
            vec![
                grant.eco.to_string(),
                grant.package.clone(),
                grant.versions.clone(),
            ]
        })
        .collect();
    table(&["ECO", "PACKAGE", "VERSIONS"], &rows)
}

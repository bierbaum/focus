use std::{path::Path, sync::Arc};

use anyhow::{Context, Result};
use focus_util::app::App;
use tracing::info;

use crate::model::{repo::Repo, selection::*};

fn mutate(
    sparse_repo: impl AsRef<Path>,
    sync_if_changed: bool,
    action: OperationAction,
    projects_and_targets: Vec<String>,
    app: Arc<focus_util::app::App>,
) -> Result<()> {
    let repo = Repo::open(sparse_repo.as_ref(), app.clone())?;
    let mut selections = repo.selection_manager()?;
    if selections.mutate(action, &projects_and_targets)? {
        selections.save().context("Saving selection")?;
        if sync_if_changed {
            info!("Synchronizing after selection changed");
            super::sync::run(sparse_repo.as_ref(), app, true)?;
        }
    }

    Ok(())
}

pub fn add(
    sparse_repo: impl AsRef<Path>,
    sync_if_changed: bool,
    projects_and_targets: Vec<String>,
    app: Arc<App>,
) -> Result<()> {
    mutate(
        sparse_repo,
        sync_if_changed,
        OperationAction::Add,
        projects_and_targets,
        app,
    )
}

pub fn remove(
    sparse_repo: impl AsRef<Path>,
    sync_if_changed: bool,
    projects_and_targets: Vec<String>,
    app: Arc<App>,
) -> Result<()> {
    mutate(
        sparse_repo,
        sync_if_changed,
        OperationAction::Remove,
        projects_and_targets,
        app,
    )
}

pub fn status(sparse_repo: impl AsRef<Path>, app: Arc<App>) -> Result<()> {
    let repo = Repo::open(sparse_repo.as_ref(), app)?;
    let selections = repo.selection_manager()?;
    let selection = selections.selection()?;
    println!("{}", selection);
    Ok(())
}

pub fn list_projects(sparse_repo: impl AsRef<Path>, app: Arc<App>) -> Result<()> {
    let repo = Repo::open(sparse_repo.as_ref(), app)?;
    let selections = repo.selection_manager()?;
    println!("{}", selections.project_catalog().optional_projects);
    Ok(())
}

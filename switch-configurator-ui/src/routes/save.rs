use askama::Template;
use axum::extract::{Path, State};
use axum::response::{IntoResponse, Redirect};
use axum::Form;
use serde::Deserialize;

use super::AppState;

#[derive(Template)]
#[template(path = "partials/save_dialog.html")]
struct SaveDialogTemplate {
    switch_id: String,
    default_filename: String,
    default_priority: u16,
    error: Option<String>,
}

pub async fn save_dialog(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> impl IntoResponse {
    let has_draft = state.drafts.has_draft(&id).await;
    if !has_draft {
        return Redirect::to(&format!("/switch/{}", id)).into_response();
    }

    SaveDialogTemplate {
        switch_id: id.clone(),
        default_filename: format!("{}.yaml", id),
        default_priority: 200,
        error: None,
    }.into_response()
}

#[derive(Deserialize)]
pub struct SaveForm {
    pub filename: String,
    pub priority: u16,
}

pub async fn save_overlay(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Form(form): Form<SaveForm>,
) -> impl IntoResponse {
    let draft = match state.drafts.get(&id).await {
        Some(d) => d,
        None => return Redirect::to(&format!("/switch/{}", id)).into_response(),
    };

    let save_body = serde_json::json!({
        "filename": form.filename,
        "merge_priority": form.priority,
        "config": {
            "switches": [{
                "id": id,
                "hostname": draft.edited.hostname,
                "vlans": draft.edited.vlans,
                "ports": draft.edited.ports,
                "port_mirrors": draft.edited.port_mirrors,
                "snmp": draft.edited.snmp,
            }]
        }
    });

    match state.backend.post(&format!("/switches/{}/save-overlay", id), &save_body).await {
        Ok((status, resp)) => {
            if status >= 200 && status < 300 {
                tracing::info!("Saved overlay for {}: {:?}", id, resp);
                state.drafts.discard(&id).await;
                Redirect::to(&format!("/switch/{}", id)).into_response()
            } else {
                let error_msg = resp["error"].as_str()
                    .unwrap_or("Unknown error from backend")
                    .to_string();
                tracing::error!("Failed to save overlay: {} {}", status, error_msg);
                SaveDialogTemplate {
                    switch_id: id,
                    default_filename: form.filename,
                    default_priority: form.priority,
                    error: Some(error_msg),
                }.into_response()
            }
        }
        Err(e) => {
            tracing::error!("Failed to save overlay: {}", e);
            SaveDialogTemplate {
                switch_id: id,
                default_filename: form.filename,
                default_priority: form.priority,
                error: Some(format!("Connection error: {}", e)),
            }.into_response()
        }
    }
}

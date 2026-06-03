use axum::response::Html;

pub async fn index() -> Html<&'static str> {
    Html("<html><body><h1>Switch Configurator</h1><p>Dashboard coming soon</p></body></html>")
}

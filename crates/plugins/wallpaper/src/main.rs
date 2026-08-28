use zest_plugin_api::PluginResponse;
use zest_wallpaper_plugin::{handle, read_request};

fn main() {
    let response = match read_request().and_then(handle) {
        Ok(data) => PluginResponse::success(data),
        Err(error) => PluginResponse::failure(error),
    };

    // stdout is the protocol. Diagnostics must never be printed here because
    // Zest treats the first JSON value as the whole response.
    println!(
        "{}",
        serde_json::to_string(&response).unwrap_or_else(|_| {
            r#"{"ok":false,"data":null,"error":"The add-on could not reply."}"#.into()
        })
    );
}

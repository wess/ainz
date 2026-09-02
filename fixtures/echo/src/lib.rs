wit_bindgen::generate!({
  path: "../../wit",
  world: "plugin",
});

struct Echo;

impl Guest for Echo {
  fn call(tool: String, arguments: String) -> Result<String, String> {
    let value: serde_json::Value =
      serde_json::from_str(&arguments).map_err(|error| error.to_string())?;
    match tool.as_str() {
      "echo" => Ok(arguments),
      "read" | "denied_read" => {
        let path = value["path"].as_str().ok_or("path is required")?;
        ainz::plugin::host::read_file(path)
      }
      "write" => {
        let path = value["path"].as_str().ok_or("path is required")?;
        let content = value["content"].as_str().ok_or("content is required")?;
        ainz::plugin::host::write_file(path, content)?;
        Ok("written".into())
      }
      "run" => {
        let command = value["command"].as_str().ok_or("command is required")?;
        ainz::plugin::host::run(command)
      }
      "fetch" => {
        let url = value["url"].as_str().ok_or("url is required")?;
        ainz::plugin::host::fetch(url)
      }
      _ => Err(format!("unknown tool: {tool}")),
    }
  }
}

export!(Echo);

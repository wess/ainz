use super::*;

fn script(code: &str) -> (Arc<Script>, Config, Vec<String>, tempfile::TempDir) {
  let dir = tempfile::tempdir().unwrap();
  let init = dir.path().join("init.lua");
  std::fs::write(&init, code).unwrap();
  let sources = Sources {
    init: Some(init),
    roots: vec![dir.path().to_path_buf()],
    project: Project::None,
  };
  let mut config = Config::default();
  let (script, notes) = Script::load(sources, dir.path(), &mut config).unwrap();
  (Arc::new(script), config, notes, dir)
}

#[test]
fn init_sets_options_the_way_neovim_does() {
  let (_, config, notes, _dir) = script(
    r#"
      ainz.opt.max_steps = 64
      ainz.opt.ui.vim = true
      ainz.o.permissions = "auto"
    "#,
  );
  assert!(notes.is_empty(), "{notes:?}");
  assert_eq!(config.max_steps, 64);
  assert!(config.ui.vim);
  assert_eq!(config.permissions, crate::PermissionMode::Auto);
}

#[test]
fn a_broken_script_is_reported_and_the_options_before_it_still_apply() {
  let (_, config, notes, _dir) = script("ainz.opt.max_steps = 9\nerror('boom')");
  assert_eq!(config.max_steps, 9);
  assert!(notes.iter().any(|note| note.contains("boom")), "{notes:?}");
}

#[test]
fn a_misspelt_option_is_reported() {
  let (_, _, notes, _dir) = script("ainz.opt.max_stpes = 9");
  assert!(
    notes.iter().any(|note| note.contains("max_stpes")),
    "{notes:?}"
  );
}

#[test]
fn reads_see_the_configuration_as_it_now_stands() {
  let (script, mut config, _, _dir) = script("");
  config.model = "changed-in-settings".into();
  script.sync(&config).unwrap();
  assert_eq!(
    script.run("ainz.opt.model").unwrap(),
    "\"changed-in-settings\""
  );
}

#[test]
fn a_write_from_a_callback_reaches_the_running_session() {
  let (script, config, _, _dir) =
    script(r#"ainz.keymap.set("ctrl+g", function() ainz.opt.ui.bell = false end)"#);
  let (_, key, _) = script.keymaps().remove(0);
  let Key::Run(id) = key else {
    panic!("a function keymap runs")
  };
  script.press(id);
  let (next, problems) = script.options(&config).unwrap();
  assert!(problems.is_empty(), "{problems:?}");
  assert!(!next.ui.bell);
}

#[test]
fn a_keymap_naming_an_action_moves_its_shortcut() {
  let (_, config, notes, _dir) = script(r#"ainz.keymap.set("f5", "roster")"#);
  assert!(notes.is_empty(), "{notes:?}");
  assert_eq!(
    config
      .ui
      .keybindings
      .get(&crate::keybind::Action::Roster)
      .map(String::as_str),
    Some("f5")
  );
}

#[test]
fn print_and_notify_reach_the_buffer() {
  let (script, ..) = script(r#"print("hello", {1}) ainz.notify("careful", "warn")"#);
  let effects = script.effects();
  assert!(
    matches!(&effects[0], Effect::Notify { text, level: Level::Info } if text.starts_with("hello"))
  );
  assert!(matches!(
    &effects[1],
    Effect::Notify {
      level: Level::Warn,
      ..
    }
  ));
}

#[test]
fn commands_run_with_their_arguments() {
  let (script, ..) = script(
    r#"ainz.command("standup", function(e) ainz.send("summarise " .. e.args) end, { desc = "daily" })"#,
  );
  assert_eq!(script.commands()[0].desc, "daily");
  assert!(script.command("standup", "since monday"));
  assert!(!script.command("nothing", ""));
  assert_eq!(
    script.effects(),
    [Effect::Send("summarise since monday".into())]
  );
}

#[test]
fn prompt_handlers_rewrite_or_drop_a_prompt() {
  let (script, ..) = script(
    r#"
      ainz.on("prompt", function(e) if e.text == "drop" then return false end end)
      ainz.on("prompt", function(e) return (e.text:gsub("^tldr", "Summarise:")) end)
    "#,
  );
  assert_eq!(
    script.prompt("tldr this".into()).as_deref(),
    Some("Summarise: this")
  );
  assert_eq!(script.prompt("drop".into()), None);
}

#[test]
fn a_pre_tool_handler_refuses_a_call_with_its_reason() {
  let (script, ..) = script(
    r#"
      ainz.on("pre_tool", function(call)
        if call.name == "shell" and call.subject:match("rm %-rf") then
          return false, "not in this repository"
        end
      end)
    "#,
  );
  let gate = script.gate();
  let call = |command: &str| ToolCall {
    id: "1".into(),
    name: "shell".into(),
    arguments: serde_json::json!({ "command": command }),
  };
  assert_eq!(
    gate(&call("rm -rf build")),
    Err("not in this repository".into())
  );
  assert_eq!(gate(&call("ls")), Ok(()));
}

#[test]
fn panels_hold_styled_lines_and_close() {
  let (script, ..) = script(
    r#"
      local p = ainz.panel.open({ side = "left", width = 20, title = "notes" })
      p:set({ "%Rhot%n cold", { "plain", fg = "accent" } })
      p:append("more")
      _G.panel = p
    "#,
  );
  let panels = script.panels();
  assert_eq!(panels[0].side, Side::Left);
  assert_eq!(panels[0].title, "notes");
  assert_eq!(panels[0].lines.len(), 3);
  assert_eq!(text::plain(&panels[0].lines[0]), "hot cold");
  assert_eq!(
    panels[0].lines[1][0].style.fg,
    Some(Color::Role("accent".into()))
  );
  script.run("panel:close()").unwrap();
  assert!(script.panels().is_empty());
}

#[test]
fn status_items_and_the_whole_bar_draw_from_the_state() {
  let (script, ..) = script(
    r#"
      ainz.statusbar.item("hello", function(s) return "%G" .. s.model end)
      ainz.statusbar.set({ "hello", "mode" })
      ainz.statusbar.render(function(s) return "[" .. s.mode .. "]" end)
    "#,
  );
  script.describe(View {
    model: "m1".into(),
    mode: "ask".into(),
    ..View::default()
  });
  assert_eq!(text::plain(&script.item("hello").unwrap()), "m1");
  assert_eq!(script.bar().unwrap(), ["hello", "mode"]);
  assert_eq!(text::plain(&script.statusline().unwrap()), "[ask]");
  assert!(script.item("absent").is_none());
}

#[test]
fn timers_fire_when_due_and_repeat_until_stopped() {
  let (script, ..) = script(
    r#"
      count = 0
      ticker = ainz.timer(0, function() count = count + 1 end, { ["repeat"] = true })
      ainz.defer(0, function() ainz.notify("once") end)
    "#,
  );
  std::thread::sleep(Duration::from_millis(120));
  script.fire_timers();
  std::thread::sleep(Duration::from_millis(120));
  script.fire_timers();
  assert_eq!(script.run("count").unwrap(), "2");
  let notes = script.effects();
  assert_eq!(notes.len(), 1, "a one-shot timer fires once");
  script.run("ticker:stop()").unwrap();
  assert!(script.next_timer().is_none());
}

#[test]
fn a_callback_that_never_ends_is_stopped() {
  let (script, ..) = script(r#"ainz.command("spin", function() while true do end end)"#);
  let started = Instant::now();
  script.command("spin", "");
  assert!(started.elapsed() < Duration::from_secs(5));
  assert!(
    matches!(&script.effects()[0], Effect::Notify { level: Level::Error, text } if text.contains("too long"))
  );
}

#[test]
fn require_finds_modules_in_the_lua_folder() {
  let dir = tempfile::tempdir().unwrap();
  std::fs::create_dir_all(dir.path().join("lua/mine")).unwrap();
  std::fs::write(
    dir.path().join("lua/mine/init.lua"),
    "return { answer = 42 }",
  )
  .unwrap();
  std::fs::write(
    dir.path().join("init.lua"),
    "answer = require('mine').answer",
  )
  .unwrap();
  let sources = Sources {
    init: Some(dir.path().join("init.lua")),
    roots: vec![dir.path().to_path_buf()],
    project: Project::None,
  };
  let mut config = Config::default();
  let (script, notes) = Script::load(sources, dir.path(), &mut config).unwrap();
  assert!(notes.is_empty(), "{notes:?}");
  assert_eq!(script.run("answer").unwrap(), "42");
}

#[tokio::test]
async fn a_scripted_tool_answers_the_model() {
  let (script, ..) = script(
    r#"
      ainz.tool({
        name = "shout",
        description = "Says it louder",
        risk = "read",
        parameters = { type = "object", properties = { text = { type = "string" } } },
        run = function(args) return args.text:upper() end,
      })
    "#,
  );
  let tools = script.tools();
  assert_eq!(tools[0].spec().name, "shout");
  assert_eq!(
    tools[0].risk(&serde_json::json!({})),
    crate::tool::Risk::Read
  );
  let context = crate::tool::ToolContext::new(std::env::temp_dir(), uuid::Uuid::nil(), 1024);
  let output = tools[0]
    .execute(&context, serde_json::json!({ "text": "hi" }))
    .await
    .unwrap();
  assert_eq!(output, "HI");
}

#[test]
fn reload_drops_what_was_registered_and_runs_the_scripts_again() {
  let (script, mut config, _, dir) = script(r#"ainz.command("one", function() end)"#);
  std::fs::write(
    dir.path().join("init.lua"),
    r#"ainz.command("two", function() end)"#,
  )
  .unwrap();
  let sources = script.sources();
  let notes = script.reload(sources, &mut config);
  assert!(notes.is_empty(), "{notes:?}");
  let names: Vec<_> = script
    .commands()
    .into_iter()
    .map(|command| command.name)
    .collect();
  assert_eq!(names, ["two"]);
}

#[test]
fn the_example_init_lua_loads_cleanly() {
  let example = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/init.lua");
  let (script, _, notes, _dir) = script(&std::fs::read_to_string(example).unwrap());
  assert!(notes.is_empty(), "{notes:?}");
  assert_eq!(script.panels().len(), 1);
  assert!(script.bar().unwrap().contains(&"ctx".to_string()));
  script.describe(View {
    model: "m".into(),
    workspace: "/tmp/repo".into(),
    context: Some(super::Context {
      used: 90,
      limit: 100,
      percent: 90,
    }),
    ..View::default()
  });
  assert_eq!(text::plain(&script.item("ctx").unwrap()), "ctx 90%");
  assert!(text::plain(&script.topic().unwrap()).starts_with("#repo"));
  assert!(script.effects().is_empty(), "{:?}", script.effects());
}

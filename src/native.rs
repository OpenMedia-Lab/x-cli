use base64::Engine;
use clap::{Arg, ArgAction, Command};
use serde_json::{Value, json};
use x_api::backend::Backend;
use x_api::web::WebBackend;

pub(crate) fn command() -> Command {
    let image = || {
        Arg::new("image")
            .long("image")
            .action(ArgAction::Append)
            .help("Image path (repeat up to four times)")
    };
    let count = || {
        Arg::new("count")
            .long("count")
            .default_value("20")
            .value_parser(clap::value_parser!(u32).range(1..=100))
    };
    let cursor = || Arg::new("cursor").long("cursor");
    Command::new("native")
        .about("Browser-session X operations; outputs JSON")
        .subcommand_required(true)
        .subcommand(Command::new("login").about("Official browser login; --qr uses browser/OS Passkey QR when available").arg(Arg::new("qr").long("qr").action(ArgAction::SetTrue).help("Choose Passkey and a nearby device in the browser; requires an enrolled Passkey")).arg(Arg::new("timeout").long("timeout").default_value("180").value_parser(clap::value_parser!(u64).range(1..=600))))
        .subcommand(Command::new("me"))
        .subcommand(Command::new("user").arg(Arg::new("username").required(true)))
        .subcommand(Command::new("read").arg(Arg::new("id").required(true)))
        .subcommand(
            Command::new("search")
                .arg(Arg::new("query").required(true))
                .arg(count())
                .arg(cursor()),
        )
        .subcommand(
            Command::new("timeline")
                .arg(Arg::new("username").required(true))
                .arg(count())
                .arg(cursor()),
        )
        .subcommand(
            Command::new("post")
                .arg(Arg::new("text").default_value(""))
                .arg(image()),
        )
        .subcommand(
            Command::new("reply")
                .arg(Arg::new("id").required(true))
                .arg(Arg::new("text").required(true))
                .arg(image()),
        )
        .subcommand(Command::new("delete").arg(Arg::new("id").required(true)))
}

pub(crate) fn run(args: Vec<std::ffi::OsString>) -> i32 {
    let matches = match command().try_get_matches_from(args) {
        Ok(matches) => matches,
        Err(error) => {
            let code = error.exit_code();
            if code == 0 {
                let _ = error.print();
            } else {
                eprintln!(
                    "{}",
                    json!({"error":error.to_string(), "operation":"parse"})
                );
            }
            return code;
        }
    };
    let (operation, options) = matches.subcommand().unwrap();
    let images: Vec<String> = if matches!(operation, "post" | "reply") {
        options
            .get_many::<String>("image")
            .map(|v| v.cloned().collect())
            .unwrap_or_default()
    } else {
        vec![]
    };
    let execute = || -> Result<Value, Box<dyn std::error::Error>> {
        if operation == "login" {
            return crate::login::run(
                options.get_flag("qr"),
                *options.get_one::<u64>("timeout").unwrap(),
            );
        }
        if images.len() > 4 {
            return Err("X supports at most four images per post".into());
        }
        let text = if matches!(operation, "post" | "reply") {
            options
                .get_one::<String>("text")
                .cloned()
                .unwrap_or_default()
        } else {
            String::new()
        };
        if operation == "post" && text.trim().is_empty() && images.is_empty() {
            return Err("post requires text or an image".into());
        }
        // Read all paths before any upload or post, so bad local inputs cannot partially publish.
        let media: Vec<Vec<u8>> = images.iter().map(std::fs::read).collect::<Result<_, _>>()?;
        let mut backend = WebBackend::new()?;
        let value = match operation {
            "me" => backend.native_me()?,
            "user" => backend.native_user(options.get_one::<String>("username").unwrap())?,
            "read" => backend.native_read(options.get_one::<String>("id").unwrap())?,
            "search" | "timeline" => {
                let count = *options.get_one::<u32>("count").unwrap();
                let cursor = options.get_one::<String>("cursor").map(String::as_str);
                if operation == "search" {
                    backend.native_search(
                        options.get_one::<String>("query").unwrap(),
                        count,
                        cursor,
                    )?
                } else {
                    backend.native_timeline(
                        options.get_one::<String>("username").unwrap(),
                        count,
                        cursor,
                    )?
                }
            }
            "post" | "reply" => {
                let mut ids = vec![];
                for bytes in media {
                    let uploaded = backend.post_json_body(
                        "/2/media/upload",
                        json!({"media":base64::engine::general_purpose::STANDARD.encode(bytes)}),
                    )?;
                    ids.push(uploaded["data"]["id"].clone());
                }
                let mut body = json!({"text":text,"media":{"media_ids":ids}});
                if operation == "reply" {
                    body["reply"] =
                        json!({"in_reply_to_tweet_id":options.get_one::<String>("id").unwrap()});
                }
                backend.post_json_body("/2/tweets", body)?
            }
            "delete" => backend.delete_json(
                &format!("/2/tweets/{}", options.get_one::<String>("id").unwrap()),
                vec![],
            )?,
            _ => unreachable!(),
        };
        Ok(value)
    };
    match execute() {
        Ok(value) => {
            println!("{}", serde_json::to_string(&value).unwrap());
            0
        }
        Err(error) => {
            eprintln!(
                "{}",
                json!({"error":error.to_string(),"operation":operation})
            );
            1
        }
    }
}

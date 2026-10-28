use anyhow::Result;
use serde_json::json;

use crate::api::issues::{self, parse_key};
use crate::cli::TransitionArgs;
use crate::context::Ctx;
use crate::output::{print_json, table};

pub fn run(ctx: &Ctx, args: TransitionArgs) -> Result<()> {
    let key = parse_key(&args.key)?;
    let transitions = issues::transitions(&ctx.client, &key)?;
    let Some(status) = args.status.filter(|_| !args.list) else {
        if ctx.json {
            return print_json(&transitions);
        }
        if transitions.is_empty() {
            println!("{key} has no transitions available.");
            return Ok(());
        }
        let rows: Vec<Vec<String>> = transitions
            .iter()
            .map(|t| vec![t.id.clone(), t.name.clone(), format!("→ {}", t.to)])
            .collect();
        println!("{}", table(&rows, ""));
        return Ok(());
    };
    let transition = issues::find_transition(&transitions, &status)?;
    let before = issues::fetch(&ctx.client, &key, "status", "")?["fields"]["status"]["name"]
        .as_str()
        .unwrap_or("?")
        .to_string();
    issues::transition(&ctx.client, &key, &transition.id)?;
    if ctx.json {
        return print_json(&json!({
            "key": key,
            "from": before,
            "to": transition.to,
            "transition": transition.name,
        }));
    }
    println!("{key}: {before} → {}", transition.to);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::Client;
    use crate::credentials::{Auth, Credentials, Source};
    use mockito::{Matcher, Server};

    fn ctx(server: &Server, json: bool) -> Ctx {
        Ctx {
            client: Client::for_tests(&server.url()),
            creds: Credentials {
                auth: Auth::Bearer("t".into()),
                api: server.url(),
                site: "x.atlassian.net".into(),
                cloud_id: None,
                email: None,
                expiry: None,
                source: Source::Env(vec![]),
            },
            site: None,
            json,
            yes: true,
        }
    }

    #[test]
    fn moves_the_issue_through_the_named_transition() {
        let mut server = Server::new();
        server
            .mock("GET", "/issue/A-1/transitions")
            .with_body(r#"{"transitions": [{"id": "31", "name": "Close Issue", "to": {"name": "Closed"}}]}"#)
            .expect(2)
            .create();
        server
            .mock("GET", "/issue/A-1")
            .match_query(Matcher::UrlEncoded("fields".into(), "status".into()))
            .with_body(r#"{"fields": {"status": {"name": "Open"}}}"#)
            .create();
        let post = server
            .mock("POST", "/issue/A-1/transitions")
            .match_body(Matcher::Json(json!({"transition": {"id": "31"}})))
            .with_status(204)
            .create();
        let args = TransitionArgs {
            key: "a-1".into(),
            status: Some("closed".into()),
            list: false,
        };
        run(&ctx(&server, true), args).unwrap();
        post.assert();

        let listing = TransitionArgs {
            key: "A-1".into(),
            status: None,
            list: true,
        };
        run(&ctx(&server, false), listing).unwrap();
    }
}

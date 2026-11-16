//! Handler tests: drive the real router in-process with `tower::ServiceExt::oneshot`.

use std::sync::Arc;

use axum::Router;
use axum::body::Body;
use axum::http::{Request, Response, StatusCode, header};
use choosey_store::MemoryStore;
use choosey_web::app::{AppState, router};
use choosey_web::config::Config;
use choosey_web::notify::NoopNotifier;
use http_body_util::BodyExt;
use tower::ServiceExt;

fn app_with(config: Config) -> Router {
    router(AppState {
        store: Arc::new(MemoryStore::new()),
        config: Arc::new(config),
        notifier: Arc::new(NoopNotifier),
    })
}

fn app() -> Router {
    app_with(Config::for_tests())
}

async fn send(app: &Router, req: Request<Body>) -> Response<Body> {
    app.clone().oneshot(req).await.unwrap()
}

async fn get(app: &Router, uri: &str) -> Response<Body> {
    send(app, Request::get(uri).body(Body::empty()).unwrap()).await
}

async fn post(app: &Router, uri: &str, form: &str) -> Response<Body> {
    let req = Request::post(uri)
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(Body::from(form.to_string()))
        .unwrap();
    send(app, req).await
}

async fn body(res: Response<Body>) -> String {
    let bytes = res.into_body().collect().await.unwrap().to_bytes();
    String::from_utf8(bytes.to_vec()).unwrap()
}

fn location(res: &Response<Body>) -> String {
    res.headers()[header::LOCATION]
        .to_str()
        .unwrap()
        .to_string()
}

fn header_of<'a>(res: &'a Response<Body>, name: &str) -> &'a str {
    res.headers()
        .get(name)
        .map(|v| v.to_str().unwrap())
        .unwrap_or("")
}

/// Create a decision and return its edit path (without `?new=1`).
async fn create(app: &Router, form: &str) -> String {
    let res = post(app, "/d", form).await;
    assert_eq!(res.status(), StatusCode::SEE_OTHER);
    location(&res).trim_end_matches("?new=1").to_string()
}

fn view_path(edit: &str) -> String {
    edit.split("/edit/").next().unwrap().to_string()
}

/// The page must be clean under our CSP: no inline scripts, styles or handlers.
fn assert_csp_clean(html: &str) {
    assert!(!html.contains("style="), "inline style attribute");
    assert!(!html.contains("<style"), "inline <style>");
    assert!(!html.contains("hx-on"), "inline htmx handler");
    for (i, _) in html.match_indices("<script") {
        let tag = &html[i..i + html[i..].find('>').unwrap()];
        assert!(tag.contains("src="), "inline <script>: {tag}");
    }
    // Inline event handlers look like ` onclick=`.
    for (i, _) in html.match_indices(" on") {
        let rest = &html[i + 3..];
        let word: String = rest
            .chars()
            .take_while(|c| c.is_ascii_alphabetic())
            .collect();
        assert!(
            !rest[word.len()..].starts_with('='),
            "inline event handler on{word}"
        );
    }
}

#[tokio::test]
async fn home_has_security_headers() {
    let app = app();
    let res = get(&app, "/").await;
    assert_eq!(res.status(), StatusCode::OK);
    let csp = header_of(&res, "content-security-policy");
    assert!(csp.contains("default-src 'self'") && csp.contains("frame-ancestors 'none'"));
    assert_eq!(header_of(&res, "x-content-type-options"), "nosniff");
    assert_eq!(header_of(&res, "referrer-policy"), "no-referrer");
    assert!(header_of(&res, "permissions-policy").contains("camera=()"));
    assert_eq!(header_of(&res, "cache-control"), "no-store");
    // Local config is http, so no HSTS.
    assert_eq!(header_of(&res, "strict-transport-security"), "");
    let html = body(res).await;
    assert!(html.contains("includeIndicatorStyles\":false"));
    assert_csp_clean(&html);
}

#[tokio::test]
async fn hsts_when_public_url_is_https() {
    let mut config = Config::for_tests();
    config.hsts = true;
    let res = get(&app_with(config), "/").await;
    assert!(header_of(&res, "strict-transport-security").contains("max-age=63072000"));
}

#[tokio::test]
async fn static_assets_are_cacheable() {
    let app = app();
    let html = body(get(&app, "/").await).await;
    let css = html
        .split('"')
        .find(|s| s.starts_with("/static/app."))
        .unwrap()
        .to_string();
    let res = get(&app, &css).await;
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(header_of(&res, "cache-control"), "public, max-age=86400");
    assert!(header_of(&res, "content-type").starts_with("text/css"));
    let res = get(&app, "/static/htmx-2.0.11.min.js").await;
    assert_eq!(res.status(), StatusCode::OK);
    let res = get(&app, "/static/nope.js").await;
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
    assert_eq!(header_of(&res, "cache-control"), "no-store");
}

#[tokio::test]
async fn full_flow_without_javascript() {
    let app = app();
    let res = post(&app, "/d", "title=Which+flat%3F&template=home").await;
    let edit = location(&res);
    assert!(edit.ends_with("?new=1"));
    let page = body(get(&app, &edit).await).await;
    assert!(page.contains("Bookmark this link. It's the only way back in."));
    assert!(page.contains("Which flat?"));
    assert_csp_clean(&page);
    let edit = edit.trim_end_matches("?new=1").to_string();

    // Options.
    for name in ["Elm+Street", "Park+View"] {
        let res = post(
            &app,
            &format!("{edit}/options"),
            &format!("name={name}&notes="),
        )
        .await;
        assert_eq!(res.status(), StatusCode::SEE_OTHER);
        assert!(location(&res).starts_with(&format!("{edit}/options?added=o")));
    }
    // The home template has 9 criteria (c1..c9), so options are o10 and o11.
    // c1 = Price (numeric, lower is better, no bounds), c3 = Sun (rated).
    let res = post(
        &app,
        &format!("{edit}/scores"),
        "option=o11&s.o10.c1=1500&s.o11.c1=2000&s.o10.c3=4&s.o11.c3=9",
    )
    .await;
    assert_eq!(location(&res), format!("{edit}/score?option=o11"));

    // Only Price (10) and Sun (5) count: Elm = (10*10 + 5*4)/150 = 80, Park = (0 + 45)/150 = 30.
    let res = post(
        &app,
        &format!("{edit}/weights"),
        "w.c1=10&w.c2=0&w.c3=5&w.c4=0&w.c5=0&w.c6=0&w.c7=0&w.c8=0&w.c9=0",
    )
    .await;
    assert_eq!(res.status(), StatusCode::SEE_OTHER);
    let results = body(get(&app, &format!("{edit}/results")).await).await;
    assert!(
        results.contains("<span class=\"total\">80</span>"),
        "{results}"
    );
    assert!(results.contains("<span class=\"total\">30</span>"));
    assert!(!results.contains("Incomplete"));
    assert!(results.contains("<rect class=\"seg seg-0\""));
    assert_csp_clean(&results);

    // A missing score shows the incomplete badge rather than counting as 0.
    post(&app, &format!("{edit}/scores"), "s.o11.c3=").await;
    let results = body(get(&app, &format!("{edit}/results")).await).await;
    assert!(
        results.contains("Incomplete: 67% of the weight scored"),
        "{results}"
    );

    // Read-only view and CSV.
    let view = view_path(&edit);
    let res = get(&app, &view).await;
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(header_of(&res, "x-robots-tag"), "noindex");
    let html = body(res).await;
    assert!(html.contains("Read-only view") && !html.contains("/edit/"));
    let res = get(&app, &format!("{view}/export.csv")).await;
    assert!(header_of(&res, "content-disposition").starts_with("attachment"));
    let csv = body(res).await;
    assert!(csv.starts_with("Rank,Option"));
    assert!(csv.contains("1,Elm Street,,80,100,1500,10,"), "{csv}");
}

#[tokio::test]
async fn blank_decision_criteria_and_ranking() {
    let app = app();
    let edit = create(&app, "template=blank").await;
    let page = body(get(&app, &edit).await).await;
    assert!(page.contains("My decision") && page.contains("No criteria yet"));

    post(
        &app,
        &format!("{edit}/criteria"),
        "name=Pay&kind=higher&unit=per+year",
    )
    .await;
    post(&app, &format!("{edit}/criteria"), "name=Team&kind=rated").await;
    let res = post(
        &app,
        &format!("{edit}/criteria"),
        "name=Commute&kind=lower&unit=min&worst=60&best=10",
    )
    .await;
    assert_eq!(res.status(), StatusCode::SEE_OTHER);

    let page = body(get(&app, &edit).await).await;
    // Numeric without bounds gets the min-max hint, once.
    assert_eq!(
        page.matches("can make small differences look big").count(),
        1
    );

    // Rank: Team first, Pay and Commute tied second -> 10, 5, 5 (see core::rank_weights tests).
    let res = post(&app, &format!("{edit}/rank"), "r.c1=2&r.c2=1&r.c3=2").await;
    assert_eq!(res.status(), StatusCode::SEE_OTHER);
    let page = body(get(&app, &edit).await).await;
    assert!(page.contains(
        "id=\"w-c1\" name=\"w.c1\" type=\"range\" min=\"0\" max=\"10\" step=\"1\" value=\"5\""
    ));
    assert!(
        page.contains("name=\"w.c2\" type=\"range\" min=\"0\" max=\"10\" step=\"1\" value=\"10\"")
    );

    // Move Commute up in rank mode.
    let res = post(&app, &format!("{edit}/criteria/c3/up"), "").await;
    assert_eq!(location(&res), format!("{edit}?mode=rank"));
    let page = body(get(&app, &format!("{edit}?mode=rank")).await).await;
    let team = page.find("r-c2").unwrap();
    let commute = page.find("r-c3").unwrap();
    assert!(commute < team);

    // Changing a criterion from numeric to rated clears its scores.
    post(&app, &format!("{edit}/options"), "name=A").await; // o4
    post(&app, &format!("{edit}/scores"), "s.o4.c1=50000").await;
    post(&app, &format!("{edit}/criteria/c1"), "name=Pay&kind=rated").await;
    let page = body(get(&app, &format!("{edit}/score")).await).await;
    assert!(page.contains("name=\"s.o4.c1\" type=\"number\" inputmode=\"decimal\" min=\"0\" max=\"10\" step=\"0.5\" value=\"\""));

    // Delete a criterion and an option.
    post(&app, &format!("{edit}/criteria/c2/delete"), "").await;
    post(&app, &format!("{edit}/options/o4/delete"), "").await;
    let page = body(get(&app, &format!("{edit}/options")).await).await;
    assert!(page.contains("No options yet"));
}

#[tokio::test]
async fn validation_errors_are_shown_and_nothing_is_saved() {
    let app = app();
    let edit = create(&app, "template=blank").await;
    let long = "x".repeat(101);
    let cases = [
        ("criteria", format!("name={long}&kind=rated"), "at most 100"),
        ("criteria", "name=+++&kind=rated".to_string(), "be empty"),
        ("criteria", "name=A&kind=weird".to_string(), "Choose how"),
        (
            "criteria",
            "name=A&kind=higher&worst=5".to_string(),
            "Set both",
        ),
        (
            "criteria",
            "name=A&kind=higher&worst=10&best=5".to_string(),
            "larger number",
        ),
        (
            "criteria",
            "name=A&kind=lower&worst=inf&best=5".to_string(),
            "out of range",
        ),
        ("options", "name=".to_string(), "be empty"),
    ];
    for (path, form, msg) in cases {
        let res = post(&app, &format!("{edit}/{path}"), &form).await;
        assert_eq!(res.status(), StatusCode::UNPROCESSABLE_ENTITY, "{form}");
        assert!(body(res).await.contains(msg), "{form} should say {msg}");
    }
    post(&app, &format!("{edit}/criteria"), "name=Sun&kind=rated").await; // c1
    post(&app, &format!("{edit}/options"), "name=A").await; // o2
    for bad in ["s.o2.c1=11", "s.o2.c1=7.3", "s.o2.c1=NaN", "s.o2.c1=abc"] {
        let res = post(&app, &format!("{edit}/scores"), bad).await;
        assert_eq!(res.status(), StatusCode::UNPROCESSABLE_ENTITY, "{bad}");
    }
    let res = post(&app, &format!("{edit}/weights"), "w.c1=11").await;
    assert_eq!(res.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let page = body(get(&app, &edit).await).await;
    assert!(page.contains("value=\"5\""), "weight unchanged");

    // Names are trimmed and control characters stripped; HTML is escaped on output.
    post(
        &app,
        &format!("{edit}/options"),
        "name=%3Cb%3Ebold%3C%2Fb%3E%00+",
    )
    .await;
    let page = body(get(&app, &format!("{edit}/options")).await).await;
    assert!(page.contains("&#60;b&#62;bold&#60;/b&#62;"));
    assert!(!page.contains("<b>bold"));
}

#[tokio::test]
async fn wrong_secret_and_unknown_decision_look_the_same() {
    let app = app();
    let edit = create(&app, "template=job").await;
    let view = view_path(&edit);
    let wrong = format!("{view}/edit/{}", "A".repeat(22));
    let unknown = format!("/d/{}/edit/{}", "B".repeat(22), "A".repeat(22));
    let a = get(&app, &wrong).await;
    let b = get(&app, &unknown).await;
    assert_eq!(a.status(), StatusCode::NOT_FOUND);
    assert_eq!(b.status(), StatusCode::NOT_FOUND);
    assert_eq!(body(a).await, body(b).await);
    // Malformed ids never reach the store, and also 404.
    for uri in [
        "/d/short",
        "/d/has-dash-aaaaaaaaaaaaaaaa",
        &format!("{view}/edit/x"),
    ] {
        assert_eq!(
            get(&app, uri).await.status(),
            StatusCode::NOT_FOUND,
            "{uri}"
        );
    }
    // Writes with a wrong secret are refused the same way.
    let res = post(&app, &format!("{wrong}/title"), "title=pwned").await;
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn delete_needs_confirmation() {
    let app = app();
    let edit = create(&app, "template=blank").await;
    let res = post(&app, &format!("{edit}/delete"), "").await;
    assert_eq!(res.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(get(&app, &edit).await.status(), StatusCode::OK);
    let res = post(&app, &format!("{edit}/delete"), "confirm=yes").await;
    assert_eq!(location(&res), "/?deleted=1");
    assert_eq!(get(&app, &edit).await.status(), StatusCode::NOT_FOUND);
    assert_eq!(
        get(&app, &view_path(&edit)).await.status(),
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn origin_secret_is_enforced_when_configured() {
    let mut config = Config::for_tests();
    config.origin_verify_secret = Some("s3cr3t-value".into());
    let app = app_with(config);
    let res = get(&app, "/").await;
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
    // Security headers are still present on the rejection.
    assert_eq!(header_of(&res, "x-content-type-options"), "nosniff");
    let bad = Request::get("/")
        .header("x-origin-verify", "nope")
        .body(Body::empty())
        .unwrap();
    assert_eq!(send(&app, bad).await.status(), StatusCode::FORBIDDEN);
    let good = Request::get("/")
        .header("x-origin-verify", "s3cr3t-value")
        .body(Body::empty())
        .unwrap();
    assert_eq!(send(&app, good).await.status(), StatusCode::OK);
}

#[tokio::test]
async fn cross_origin_posts_are_rejected() {
    let app = app();
    let edit = create(&app, "template=blank").await;
    let form = |origin: &str| {
        Request::post(format!("{edit}/title"))
            .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
            .header(header::ORIGIN, origin)
            .body(Body::from("title=Renamed"))
            .unwrap()
    };
    assert_eq!(
        send(&app, form("https://evil.example")).await.status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        send(&app, form("null")).await.status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        send(&app, form("http://127.0.0.1:3000")).await.status(),
        StatusCode::SEE_OTHER
    );
}

#[tokio::test]
async fn only_post_changes_state() {
    let app = app();
    let edit = create(&app, "template=blank").await;
    for path in ["title", "delete", "scores", "weights", "rank"] {
        let res = get(&app, &format!("{edit}/{path}")).await;
        assert_eq!(res.status(), StatusCode::METHOD_NOT_ALLOWED, "{path}");
    }
    assert_eq!(
        get(&app, "/d").await.status(),
        StatusCode::METHOD_NOT_ALLOWED
    );
}

#[tokio::test]
async fn large_bodies_are_rejected() {
    let app = app();
    let edit = create(&app, "template=blank").await;
    let big = format!("name={}", "a".repeat(70 * 1024));
    let res = post(&app, &format!("{edit}/options"), &big).await;
    assert_eq!(res.status(), StatusCode::PAYLOAD_TOO_LARGE);
}

#[tokio::test]
async fn limits_on_counts() {
    let app = app();
    let edit = create(&app, "template=blank").await;
    for i in 0..30 {
        let res = post(
            &app,
            &format!("{edit}/criteria"),
            &format!("name=c{i}&kind=rated"),
        )
        .await;
        assert_eq!(res.status(), StatusCode::SEE_OTHER);
    }
    let res = post(
        &app,
        &format!("{edit}/criteria"),
        "name=one+more&kind=rated",
    )
    .await;
    assert_eq!(res.status(), StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn htmx_errors_retarget_main() {
    let app = app();
    let edit = create(&app, "template=blank").await;
    let req = Request::post(format!("{edit}/options"))
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .header("hx-request", "true")
        .body(Body::from("name="))
        .unwrap();
    let res = send(&app, req).await;
    assert_eq!(res.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(header_of(&res, "hx-retarget"), "#main");
    assert_eq!(header_of(&res, "hx-reselect"), "#main");
}

#[tokio::test]
async fn unknown_template_is_rejected() {
    let res = post(&app(), "/d", "template=nope").await;
    assert_eq!(res.status(), StatusCode::UNPROCESSABLE_ENTITY);
}

/// Regression test: a name typed into the Add box and then "Next" pressed (instead of "Add") used
/// to be silently discarded, because "Next" was a plain link. It now submits the Add form.
#[tokio::test]
async fn next_keeps_a_typed_but_unadded_item() {
    let app = app();
    let edit = create(&app, "template=blank").await;

    // Both pages render "Next" as a submit button for their Add form, not a link.
    let criteria_page = body(get(&app, &edit).await).await;
    assert!(criteria_page.contains(r#"form="add-criterion" name="then" value="next""#));
    let options_page = body(get(&app, &format!("{edit}/options")).await).await;
    assert!(options_page.contains(r#"form="add-option" name="then" value="next""#));
    assert_csp_clean(&options_page);

    // Criteria: Next with a name typed adds it, then moves on to Options.
    let res = post(
        &app,
        &format!("{edit}/criteria"),
        "then=next&name=Access+to+surf&kind=rated",
    )
    .await;
    assert_eq!(res.status(), StatusCode::SEE_OTHER);
    assert!(location(&res).ends_with("/options"));
    assert!(
        body(get(&app, &edit).await)
            .await
            .contains("Access to surf")
    );

    // Options: Next with a name typed adds it, then moves on to Score.
    post(&app, &format!("{edit}/options"), "name=Dunedin&notes=").await;
    let res = post(
        &app,
        &format!("{edit}/options"),
        "then=next&name=Manly&notes=",
    )
    .await;
    assert_eq!(res.status(), StatusCode::SEE_OTHER);
    assert!(location(&res).ends_with("/score"));
    let options_page = body(get(&app, &format!("{edit}/options")).await).await;
    assert!(options_page.contains("Dunedin") && options_page.contains("Manly"));

    // Next with an empty Add box just moves on and changes nothing.
    let criteria_before = body(get(&app, &edit).await).await;
    let res = post(&app, &format!("{edit}/options"), "then=next&name=&notes=").await;
    assert_eq!(res.status(), StatusCode::SEE_OTHER);
    assert!(location(&res).ends_with("/score"));
    let res = post(
        &app,
        &format!("{edit}/criteria"),
        "then=next&name=+&kind=rated",
    )
    .await;
    assert!(location(&res).ends_with("/options"));
    let options_page = body(get(&app, &format!("{edit}/options")).await).await;
    assert_eq!(options_page.matches(r#"<details class="item">"#).count(), 2);
    assert_eq!(body(get(&app, &edit).await).await, criteria_before);

    // Next with invalid input stays on the page and shows the error, so nothing is lost.
    let res = post(
        &app,
        &format!("{edit}/options"),
        "then=next&name=&notes=Cheap+but+cold",
    )
    .await;
    assert_eq!(res.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert!(body(res).await.contains("be empty"));

    // A wrong secret still gets the same 404 as everywhere else, even for an empty Next.
    let bad = format!("{}/edit/{}", view_path(&edit), "A".repeat(22));
    let res = post(&app, &format!("{bad}/options"), "then=next&name=&notes=").await;
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
}

/// A notifier that remembers what it was asked to send.
#[derive(Default)]
struct RecordingNotifier {
    sent: std::sync::Mutex<Vec<choosey_core::model::Feedback>>,
}

#[async_trait::async_trait]
impl choosey_web::notify::Notifier for RecordingNotifier {
    async fn feedback(&self, feedback: &choosey_core::model::Feedback) {
        self.sent.lock().unwrap().push(feedback.clone());
    }
}

#[tokio::test]
async fn feedback_is_saved_and_notified() {
    let store = Arc::new(MemoryStore::new());
    let notifier = Arc::new(RecordingNotifier::default());
    let app = router(AppState {
        store: store.clone(),
        config: Arc::new(Config::for_tests()),
        notifier: notifier.clone(),
    });

    // Every page links to the form, and the form itself is CSP-clean.
    assert!(
        body(get(&app, "/").await)
            .await
            .contains(r#"href="/feedback""#)
    );
    let page = body(get(&app, "/feedback").await).await;
    assert!(page.contains(r#"name="message""#) && page.contains(r#"name="website""#));
    assert_csp_clean(&page);

    // A real message is saved, notified, and followed by a thank-you page.
    let res = post(
        &app,
        "/feedback",
        "message=The+score+page+is+confusing%0D%0Aplease+fix&contact=me%40example.com&website=",
    )
    .await;
    assert_eq!(res.status(), StatusCode::SEE_OTHER);
    assert_eq!(location(&res), "/feedback?sent=1");
    assert!(
        body(get(&app, "/feedback?sent=1").await)
            .await
            .contains("Thanks!")
    );
    let saved = store.feedback();
    assert_eq!(saved.len(), 1);
    assert_eq!(saved[0].message, "The score page is confusing\nplease fix");
    assert_eq!(saved[0].contact, "me@example.com");
    assert_eq!(notifier.sent.lock().unwrap().as_slice(), saved.as_slice());

    // Empty or overlong messages are rejected, keeping what was typed; nothing is saved.
    let res = post(&app, "/feedback", "message=+++&contact=Sam").await;
    assert_eq!(res.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let html = body(res).await;
    assert!(html.contains("Please write some feedback") && html.contains(r#"value="Sam""#));
    let long = "x".repeat(2001);
    let res = post(&app, "/feedback", &format!("message={long}")).await;
    assert_eq!(res.status(), StatusCode::UNPROCESSABLE_ENTITY);

    // The honeypot quietly drops bots: same thank-you redirect, nothing saved or sent.
    let res = post(
        &app,
        "/feedback",
        "message=Buy+now&website=http%3A%2F%2Fspam",
    )
    .await;
    assert_eq!(location(&res), "/feedback?sent=1");
    assert_eq!(store.feedback().len(), 1);
    assert_eq!(notifier.sent.lock().unwrap().len(), 1);

    // User text is escaped in the page, never interpreted as HTML.
    let res = post(
        &app,
        "/feedback",
        "message=+&contact=%3Cscript%3Ex%3C%2Fscript%3E",
    )
    .await;
    let html = body(res).await;
    assert!(!html.contains("<script>x") && html.contains("&#60;script&#62;x"));
}

#[tokio::test]
async fn feedback_rejects_cross_origin_posts() {
    let app = app();
    let req = Request::post("/feedback")
        .header(header::CONTENT_TYPE, "application/x-www-form-urlencoded")
        .header(header::ORIGIN, "https://evil.example")
        .body(Body::from("message=hi"))
        .unwrap();
    assert_eq!(send(&app, req).await.status(), StatusCode::FORBIDDEN);
}

/// Number of times `needle` appears in `haystack`.
fn count(haystack: &str, needle: &str) -> usize {
    haystack.matches(needle).count()
}

/// In rank mode each row shows the weight it would get before "Apply" is pressed, recomputed on
/// every render: after moving a criterion, and when previewing typed ties.
#[tokio::test]
async fn rank_mode_previews_suggested_weights() {
    let app = app();
    let edit = create(&app, "template=blank").await;
    for name in [
        "Cost",
        "Family",
        "Climate",
        "Community",
        "Walkability",
        "Healthcare",
        "Fun",
    ] {
        post(
            &app,
            &format!("{edit}/criteria"),
            &format!("name={name}&kind=rated"),
        )
        .await;
    }

    // Every new criterion starts at weight 5. Seven in order suggest 10, 9, 7, 6, 4, 3, 1.
    let page = body(get(&app, &format!("{edit}?mode=rank")).await).await;
    assert_csp_clean(&page);
    let shown: Vec<&str> = page
        .match_indices("now 5 → suggested <strong>")
        .map(|(i, m)| {
            let rest = &page[i + m.len()..];
            &rest[..rest.find('<').unwrap()]
        })
        .collect();
    assert_eq!(shown, ["10", "9", "7", "6", "4", "3", "1"]);

    // Moving Fun (c7) up re-renders the page with the suggestion for the new order.
    post(&app, &format!("{edit}/criteria/c7/up"), "").await;
    let page = body(get(&app, &format!("{edit}?mode=rank")).await).await;
    let fun = &page[page.find("r-c7").unwrap()..];
    assert!(fun.contains(
        "Fun\n          <span class=\"rank-weights muted\">now 5 → suggested <strong>3</strong>"
    ));

    // "Update suggestions" sends typed ranks back as a GET: ties share the average, nothing is
    // saved, and the typed numbers stay in the boxes.
    let preview = format!("{edit}?mode=rank&r.c1=1&r.c2=1&r.c3=2&r.c4=3&r.c5=4&r.c6=5&r.c7=6");
    let page = body(get(&app, &preview).await).await;
    // Cost and Family tied first: (7 + 6) / 2 = 6.5 is the top -> 10; Climate 5 / 6.5 -> 8.
    assert_eq!(count(&page, "suggested <strong>10</strong>"), 2);
    assert!(
        page.contains(
            r#"name="r.c2" type="number" inputmode="numeric" min="1" max="30" value="1""#
        )
    );
    assert_eq!(count(&page, "now 5 →"), 7, "nothing was saved");
    // The button that does it is a GET back to this page; the form carries mode=rank.
    assert!(page.contains(&format!(
        r#"formmethod="get" formaction="{edit}">Update suggestions"#
    )));
    assert!(page.contains(r#"<input type="hidden" name="mode" value="rank">"#));

    // Bad numbers in a preview are ignored (the current order is shown), not an error.
    let res = get(&app, &format!("{edit}?mode=rank&r.c1=abc")).await;
    assert_eq!(res.status(), StatusCode::OK);
    assert!(body(res).await.contains("suggested <strong>10</strong>"));

    // Applying the tied ranks sets exactly the previewed weights.
    let res = post(
        &app,
        &format!("{edit}/rank"),
        "mode=rank&r.c1=1&r.c2=1&r.c3=2&r.c4=3&r.c5=4&r.c6=5&r.c7=6",
    )
    .await;
    assert_eq!(res.status(), StatusCode::SEE_OTHER);
    let page = body(get(&app, &format!("{edit}?mode=rank")).await).await;
    assert_eq!(count(&page, "now 10 → suggested"), 2);
}

/// Every link box has a Copy button, hidden until our script reveals it, and the script is served
/// as a separate, cacheable file (the CSP allows no inline JavaScript).
#[tokio::test]
async fn link_boxes_have_copy_buttons() {
    let app = app();
    let res = post(&app, "/d", "template=blank").await;
    let new = body(get(&app, &location(&res)).await).await;
    let edit = location(&res).trim_end_matches("?new=1").to_string();
    assert!(new.contains(
        r#"<button type="button" class="secondary copy-button" data-copy="new-edit-url" aria-live="polite" hidden>Copy</button>"#
    ));
    assert_csp_clean(&new);

    let results = body(get(&app, &format!("{edit}/results")).await).await;
    assert_csp_clean(&results);
    for id in ["view-url", "edit-url"] {
        assert!(results.contains(&format!(
            r#"<input id="{id}" class="copy" type="text" readonly"#
        )));
        assert!(results.contains(&format!(
            r#"data-copy="{id}" aria-live="polite" hidden>Copy<"#
        )));
    }

    // The script is linked from every page, deferred, under a content-hashed name.
    let js = results
        .split('"')
        .find(|s| s.starts_with("/static/app.") && s.ends_with(".js"))
        .unwrap()
        .to_string();
    assert!(results.contains(&format!(r#"<script src="{js}" defer></script>"#)));
    let res = get(&app, &js).await;
    assert_eq!(res.status(), StatusCode::OK);
    assert!(header_of(&res, "content-type").starts_with("text/javascript"));
    assert_eq!(header_of(&res, "cache-control"), "public, max-age=86400");
    let script = body(res).await;
    assert!(script.contains("navigator.clipboard") && script.contains("htmx:load"));
    // htmx's own settings are unchanged.
    assert!(results.contains(r#""historyCacheSize":0"#));
}

/// After "Add", the page comes back with the cursor in the Add form's Name box and says what was
/// added, so several items can be added in a row. Nowhere else gets autofocus.
#[tokio::test]
async fn add_returns_focus_to_the_add_box() {
    let app = app();
    let edit = create(&app, "template=blank").await;

    // A fresh page load never autofocuses (it would pop up the keyboard on phones).
    for page in ["", "/options", "/score", "/results"] {
        let html = body(get(&app, &format!("{edit}{page}")).await).await;
        assert!(!html.contains("autofocus"), "{page}");
    }

    // Criteria: the redirect carries the new id, not the name.
    let res = post(
        &app,
        &format!("{edit}/criteria"),
        "name=Sea+views&kind=rated",
    )
    .await;
    assert_eq!(location(&res), format!("{edit}?added=c1"));
    let html = body(get(&app, &location(&res)).await).await;
    assert!(html.contains(r#"<p class="notice" role="status">Added Sea views.</p>"#));
    assert!(html.contains(r#"value="" placeholder="e.g. Commute" autofocus>"#));
    assert_eq!(count(&html, "autofocus"), 1, "only the Add box");
    assert_csp_clean(&html);

    // Options: the same, and the name is escaped.
    let res = post(
        &app,
        &format!("{edit}/options"),
        "name=Manly+%3Ci%3Ebeach%3C%2Fi%3E&notes=",
    )
    .await;
    assert_eq!(location(&res), format!("{edit}/options?added=o2"));
    let html = body(get(&app, &location(&res)).await).await;
    assert!(html.contains("Added Manly &#60;i&#62;beach&#60;/i&#62;."));
    assert!(html.contains(r#"placeholder="e.g. 12 Elm Street" autofocus>"#));
    assert_eq!(count(&html, "autofocus"), 1);

    // Unknown ids, or an id from the other step, show nothing and don't autofocus.
    for uri in [
        format!("{edit}/options?added=o999"),
        format!("{edit}/options?added=c1"),
        format!("{edit}?added=%3Cb%3E"),
    ] {
        let html = body(get(&app, &uri).await).await;
        assert!(
            !html.contains("Added ") && !html.contains("autofocus"),
            "{uri}"
        );
    }

    // "Next" with a typed name adds it but moves on, without the flag.
    let res = post(
        &app,
        &format!("{edit}/options"),
        "then=next&name=Bondi&notes=",
    )
    .await;
    assert_eq!(location(&res), format!("{edit}/score"));

    // A validation error re-shows the form with the message, not the confirmation.
    let res = post(&app, &format!("{edit}/options"), "name=").await;
    assert_eq!(res.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert!(!body(res).await.contains("Added "));
}

/// Regression test: on phones the Score step shows one option at a time, and its Previous/Next
/// were plain links, so scores typed with JavaScript off were lost. They now submit the form.
#[tokio::test]
async fn score_previous_next_save_before_moving() {
    let app = app();
    let edit = create(&app, "template=blank").await;
    post(&app, &format!("{edit}/criteria"), "name=Sun&kind=rated").await; // c1
    for name in ["A", "B", "C"] {
        post(&app, &format!("{edit}/options"), &format!("name={name}")).await; // o2, o3, o4
    }

    // The middle option has both buttons; each submits the scores form with its target in `go`.
    let page = body(get(&app, &format!("{edit}/score?option=o3")).await).await;
    assert_csp_clean(&page);
    assert!(page.contains("Option 2 of 3"));
    let prev = r#"<button type="submit" class="secondary" form="scores" name="go" value="o2""#;
    let next = r#"<button type="submit" class="secondary" form="scores" name="go" value="o4""#;
    assert!(page.contains(prev) && page.contains(next));
    assert!(!page.contains("score?option="), "no plain links any more");
    // With JS they post the form themselves and swap the whole page, outside the <form> so they
    // don't inherit its live-results target, waiting for any auto-save in flight.
    let at = page.find(next).unwrap();
    let tag = &page[at..at + page[at..].find('>').unwrap()];
    for attr in [
        format!(r#"hx-post="{edit}/scores""#),
        r#"hx-target="body""#.to_string(),
        r#"hx-push-url="true""#.to_string(),
        r##"hx-sync="#scores:queue last""##.to_string(),
    ] {
        assert!(tag.contains(&attr), "{attr} in {tag}");
    }
    let form = page.find(r#"<form id="scores""#).unwrap();
    assert!(at < form, "nav buttons live outside the form");
    // Enter in a score box must save, not jump: the first submit button for the form is a
    // plain save button, ahead of Previous/Next.
    let default = page
        .find(r#"<button type="submit" form="scores" class="visually-hidden""#)
        .unwrap();
    assert!(default < page.find(prev).unwrap());

    // No-JS "Next": the typed score is saved, then the next option is shown.
    let res = post(
        &app,
        &format!("{edit}/scores"),
        "option=o3&s.o3.c1=7.5&go=o4",
    )
    .await;
    assert_eq!(res.status(), StatusCode::SEE_OTHER);
    assert_eq!(location(&res), format!("{edit}/score?option=o4"));
    let page = body(get(&app, &location(&res)).await).await;
    assert!(page.contains("Option 3 of 3"));
    assert!(page.contains(r#"name="s.o3.c1" type="number" inputmode="decimal" min="0" max="10" step="0.5" value="7.5""#));

    // "Previous" works the same way.
    let res = post(&app, &format!("{edit}/scores"), "option=o4&s.o4.c1=2&go=o3").await;
    assert_eq!(location(&res), format!("{edit}/score?option=o3"));

    // An unknown or malicious target falls back to the first option.
    for go in ["o999", "c1", "%2F%2Fevil.example", ""] {
        let res = post(
            &app,
            &format!("{edit}/scores"),
            &format!("option=o3&go={go}"),
        )
        .await;
        assert_eq!(location(&res), format!("{edit}/score?option=o2"), "{go}");
    }

    // An invalid score isn't saved and the error is shown on the option being edited.
    let res = post(
        &app,
        &format!("{edit}/scores"),
        "option=o4&s.o4.c1=11&go=o2",
    )
    .await;
    assert_eq!(res.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let html = body(res).await;
    assert!(html.contains("Option 3 of 3") && html.contains(r#"role="alert""#));

    // Without `go` ("Save scores" or htmx's auto-save) it stays on the current option.
    let res = post(&app, &format!("{edit}/scores"), "option=o4&s.o4.c1=3").await;
    assert_eq!(location(&res), format!("{edit}/score?option=o4"));
}

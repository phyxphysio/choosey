//! Handler tests: drive the real router in-process with `tower::ServiceExt::oneshot`.

use std::sync::Arc;

use axum::Router;
use axum::body::Body;
use axum::http::{Request, Response, StatusCode, header};
use choosey_store::MemoryStore;
use choosey_web::app::{AppState, router};
use choosey_web::config::Config;
use http_body_util::BodyExt;
use tower::ServiceExt;

fn app_with(config: Config) -> Router {
    router(AppState {
        store: Arc::new(MemoryStore::new()),
        config: Arc::new(config),
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
        assert_eq!(location(&res), format!("{edit}/options"));
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

    // Rank: Team first, Pay and Commute tied second -> 10, 3, 3 (see core::roc tests).
    let res = post(&app, &format!("{edit}/rank"), "r.c1=2&r.c2=1&r.c3=2").await;
    assert_eq!(res.status(), StatusCode::SEE_OTHER);
    let page = body(get(&app, &edit).await).await;
    assert!(page.contains(
        "id=\"w-c1\" name=\"w.c1\" type=\"range\" min=\"0\" max=\"10\" step=\"1\" value=\"3\""
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

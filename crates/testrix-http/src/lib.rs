//! Asynchronous HTTP execution engine for Testrix built on Reqwest and Tokio.

mod error;
mod local_share;
mod runner;
mod share;

pub use error::HttpError;
pub use local_share::{
    LocalShareHandle, NetworkAdapterIp, list_outbound_ipv4_addresses, start_local_share_server,
};
pub use runner::HttpRunner;
pub use share::{
    ShareChannel, ShareFile, ShareLoginResult, ShareUploadResult, collect_project_yaml_files,
    dispatch_share_future, login_testrix_server, revoke_cloudflare_r2_share,
    revoke_testrix_server_share, upload_to_cloudflare_r2, upload_to_testrix_server,
};
pub use tokio_util::sync::CancellationToken;

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use testrix_core::{
        AuthConfig, EnvVariable, EnvironmentDoc, FolderDoc, FolderNode, HttpMethod, HttpRequestDoc,
        PreparedBody, PreparedRequest, ProjectDoc, ProjectNode, RequestBody, RequestNode,
        materialize_request,
    };
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    use super::*;

    #[test]
    fn executes_materialized_request_against_local_http_server() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        let (addr, server_handle) = rt.block_on(async {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr = listener.local_addr().unwrap();
            let handle = tokio::spawn(async move {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut buf = [0u8; 4096];
                let n = socket.read(&mut buf).await.unwrap();
                let req_str = String::from_utf8_lossy(&buf[..n]).to_string();

                assert!(req_str.starts_with("POST /items?tag=alpha HTTP/1.1"));
                assert!(
                    req_str
                        .to_ascii_lowercase()
                        .contains("authorization: bearer tok-123")
                );

                let body = r#"{"ok":true,"echo":"hello"}"#;
                let resp = format!(
                    "HTTP/1.1 201 Created\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                socket.write_all(resp.as_bytes()).await.unwrap();
            });
            (addr, handle)
        });

        // Build project -> folder -> request with inherited Bearer auth and env vars.
        let mut proj_doc = ProjectDoc::new("proj", "Test Project");
        proj_doc.base_url = format!("http://{addr}");
        proj_doc.auth = AuthConfig::Bearer {
            token: "{{token}}".to_string(),
            prefix: "Bearer".to_string(),
        };

        let mut req_doc = HttpRequestDoc::new("req", "Create", HttpMethod::Post, "/items");
        req_doc.request.params = vec![testrix_core::KeyValueRow::new("tag", "{{tag_val}}")];
        req_doc.request.body = RequestBody::Json {
            content: r#"{"msg":"hello"}"#.to_string(),
        };

        let project = ProjectNode {
            doc: proj_doc,
            root_dir: PathBuf::from("."),
            manifest_path: PathBuf::from("testrix.project.yaml"),
            folders: vec![FolderNode {
                doc: FolderDoc::new("fld", "Folder"),
                dir_path: PathBuf::from("fld"),
                manifest_path: PathBuf::from("fld/_folder.yaml"),
                has_manifest: true,
                folders: vec![],
                requests: vec![RequestNode {
                    doc: req_doc,
                    file_path: PathBuf::from("fld/create.request.yaml"),
                }],
            }],
            requests: vec![],
            environments: vec![],
        };

        let mut env = EnvironmentDoc::new("env", "Local");
        env.variables = vec![
            EnvVariable::new("token", "tok-123"),
            EnvVariable::new("tag_val", "alpha"),
        ];

        let prepared = materialize_request(&project, "req", Some(&env)).unwrap();
        let runner = HttpRunner::new().unwrap();
        let token = CancellationToken::new();

        // Drive server task on rt in background thread while runner executes.
        let srv_thread = std::thread::spawn(move || {
            rt.block_on(server_handle).unwrap();
        });

        let snapshot = runner.execute_blocking(prepared, token).unwrap();
        srv_thread.join().unwrap();

        assert_eq!(snapshot.status_code, 201);
        assert!(snapshot.is_success());
        assert!(snapshot.pretty_body.contains("\"ok\": true"));
    }

    #[test]
    fn cancellation_token_cancels_before_dispatch() {
        let runner = HttpRunner::new().unwrap();
        let token = CancellationToken::new();
        token.cancel();

        let prepared = PreparedRequest {
            request_id: "req".to_string(),
            name: "Cancelled".to_string(),
            method: HttpMethod::Get,
            url: "http://127.0.0.1:9/".to_string(),
            headers: vec![],
            body: PreparedBody::None,
            settings: Default::default(),
            resolved_auth: testrix_core::ResolvedAuth {
                effective: testrix_core::EffectiveAuth::None,
                source: None,
                chain: vec![],
            },
            unresolved_variables: vec![],
        };

        let err = runner.execute_blocking(prepared, token).unwrap_err();
        assert!(matches!(err, HttpError::Cancelled));
    }

    #[test]
    fn starts_and_stops_local_share_server_on_loopback() {
        let ips = list_outbound_ipv4_addresses();
        assert!(!ips.is_empty());

        let proj_doc = ProjectDoc::new("proj", "Local Share Test");
        let project = ProjectNode {
            doc: proj_doc,
            root_dir: PathBuf::from("."),
            manifest_path: PathBuf::from("testrix.project.yaml"),
            folders: vec![],
            requests: vec![],
            environments: vec![],
        };

        let handle = start_local_share_server("127.0.0.1".to_string(), 0, project, "local-test".to_string()).unwrap();

        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let resp_text = rt.block_on(async {
            reqwest::get(format!("{}/api/project", handle.share_url))
                .await
                .unwrap()
                .text()
                .await
                .unwrap()
        });
        assert!(resp_text.contains("Local Share Test"));
        handle.stop();
    }

    #[test]
    #[ignore = "requires live Cloudflare R2 credentials via TESTRIX_R2_* env vars"]
    fn live_cloudflare_r2_sync_and_revoke() {
        let account_id = std::env::var("TESTRIX_R2_ACCOUNT").expect("TESTRIX_R2_ACCOUNT");
        let bucket = std::env::var("TESTRIX_R2_BUCKET").expect("TESTRIX_R2_BUCKET");
        let api_token = std::env::var("TESTRIX_R2_TOKEN").expect("TESTRIX_R2_TOKEN");

        let slug = "_testrix-sync-smoke";
        let files = vec![
            ShareFile {
                path: "testrix.project.yaml".to_string(),
                content: "apiVersion: testrix.io/v1\nkind: Project\nid: proj-smoke\nname: Smoke\n".to_string(),
            },
            ShareFile {
                path: "get-orders.request.yaml".to_string(),
                content: "apiVersion: testrix.io/v1\nkind: HttpRequest\nid: req-smoke\nname: Get Orders\nrequest:\n  method: GET\n  url: https://example.com/orders\n".to_string(),
            },
        ];

        let rx = dispatch_share_future(upload_to_cloudflare_r2(
            account_id.clone(),
            bucket.clone(),
            api_token.clone(),
            String::new(),
            slug.to_string(),
            String::new(),
            files,
        ));
        let upload_res = rx.blocking_recv().expect("channel").expect("upload ok");
        assert_eq!(upload_res.channel, ShareChannel::CloudflareR2);
        assert_eq!(upload_res.files_uploaded, 2);

        let rx_rev = dispatch_share_future(revoke_cloudflare_r2_share(
            account_id,
            bucket,
            api_token,
            slug.to_string(),
            vec![
                "testrix.project.yaml".to_string(),
                "get-orders.request.yaml".to_string(),
            ],
        ));
        rx_rev.blocking_recv().expect("channel").expect("revoke ok");
    }
}

// One full poll against a mock museum: pairing, inbox, decrypt, stages,
// mldata merge and consume.

#![allow(clippy::unwrap_used, reason = "test helpers")]
use std::io::Read;
use std::sync::{Arc, Mutex};

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use ente_core::crypto::{Key, SecretKey, blob, sealed};
use ente_fork_worker::config::Config;
use ente_fork_worker::identity::WorkerIdentity;
use ente_fork_worker::pipeline::{Summary, Worker};
use mockito::Matcher;
use serde_json::{Value, json};

fn b64(b: &[u8]) -> String {
    STANDARD.encode(b)
}

fn enc_json(v: &Value, key: &Key) -> (String, String) {
    let e = blob::encrypt(&serde_json::to_vec(v).unwrap(), key).unwrap();
    (b64(&e.encrypted_data), b64(e.decryption_header.as_bytes()))
}

fn enc_mldata(v: &Value, key: &Key) -> (String, String) {
    use std::io::Write;
    let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    gz.write_all(&serde_json::to_vec(v).unwrap()).unwrap();
    let e = blob::encrypt(&gz.finish().unwrap(), key).unwrap();
    (b64(&e.encrypted_data), b64(e.decryption_header.as_bytes()))
}

fn thumbnail_png() -> Vec<u8> {
    let img = image::RgbImage::from_fn(64, 48, |x, y| {
        image::Rgb([(x * 4) as u8, (y * 5) as u8, ((x + y) * 2) as u8])
    });
    let mut out = std::io::Cursor::new(Vec::new());
    img.write_to(&mut out, image::ImageFormat::Png).unwrap();
    out.into_inner()
}

#[tokio::test(flavor = "multi_thread")]
async fn one_poll_merges_x_enrich_and_consumes() {
    let mut server = mockito::Server::new_async().await;
    let worker_sk = SecretKey::generate();
    let worker_pk = b64(worker_sk.public_key().as_bytes());
    let token = "worker-token-for-user-7";

    // File 1: ready (face + clip), file 2: no mldata yet, file 3: sealed to
    // another key.
    let key1 = Key::generate();
    let key2 = Key::generate();
    let seal = |k: &Key| b64(&sealed::seal(k.as_bytes(), &worker_sk.public_key()).unwrap());
    let wrong = b64(&sealed::seal(
        Key::generate().as_bytes(),
        &SecretKey::generate().public_key(),
    )
    .unwrap());

    server
        .mock("GET", "/fork/worker/pairing")
        .match_query(Matcher::UrlEncoded("workerPublicKey".into(), worker_pk.clone()))
        .with_body(
            json!({"pairings": [{"userID": 7, "sealedToken": b64(&sealed::seal(token.as_bytes(), &worker_sk.public_key()).unwrap()), "createdAt": 1}]})
                .to_string(),
        )
        .create_async()
        .await;
    let inbox = json!({"items": [
        {"id": 10, "fileID": 1, "sealedFileKey": seal(&key1), "createdAt": 1},
        {"id": 11, "fileID": 2, "sealedFileKey": seal(&key2), "createdAt": 1},
        {"id": 12, "fileID": 3, "sealedFileKey": wrong, "createdAt": 1},
    ]});
    server
        .mock("GET", "/fork/worker-api/inbox")
        .match_header("X-Worker-Access-Token", token)
        .match_query(Matcher::UrlEncoded("sinceID".into(), "0".into()))
        .with_body(inbox.to_string())
        .create_async()
        .await;
    server
        .mock("GET", "/fork/worker-api/inbox")
        .match_query(Matcher::UrlEncoded("sinceID".into(), "12".into()))
        .with_body(r#"{"items": []}"#)
        .create_async()
        .await;

    // Parsed from text so the long float reaches the worker as written.
    let original: Value = serde_json::from_str(
        r#"{"face": {"version": 1, "client": "web", "width": 4, "height": 3, "faces": []},
            "clip": {"version": 1, "client": "web", "embedding": [0.1234567890123456789, -2.5e-7]},
            "future": {"kept": [1, 2, 3]}}"#,
    )
    .unwrap();
    let (md_data, md_header) = enc_mldata(&original, &key1);
    server
        .mock("POST", "/fork/worker-api/files/data/fetch")
        .match_body(Matcher::Json(json!({"fileIDs": [1, 2]})))
        .with_body(
            json!({"data": [{"fileID": 1, "type": "mldata", "encryptedData": md_data, "decryptionHeader": md_header, "updatedAt": 555}],
                   "pendingIndexFileIDs": [2], "errFileIDs": []})
            .to_string(),
        )
        .create_async()
        .await;

    let (meta, meta_h) = enc_json(
        &json!({"title": "transfer GB82 WEST 1234 5698 7654 32.png", "fileType": 1}),
        &key1,
    );
    let (pm, pm_h) = enc_json(&json!({"w": 1179, "h": 2556}), &key1);
    let thumb = blob::encrypt(&thumbnail_png(), &key1).unwrap();
    server
        .mock("GET", "/fork/worker-api/file-info/1")
        .with_body(
            json!({"file": {"id": 1, "ownerID": 7,
                "file": {"decryptionHeader": "unused"},
                "thumbnail": {"decryptionHeader": b64(thumb.decryption_header.as_bytes())},
                "metadata": {"encryptedData": meta, "decryptionHeader": meta_h},
                "pubMagicMetadata": {"version": 1, "count": 2, "data": pm, "header": pm_h},
                "info": {"fileSize": 10, "thumbSize": 10}}})
            .to_string(),
        )
        .create_async()
        .await;
    server
        .mock("GET", "/fork/worker-api/files/thumbnail/v3/1")
        .with_body(json!({"url": format!("{}/objects/thumb-1", server.url())}).to_string())
        .create_async()
        .await;
    server
        .mock("GET", "/objects/thumb-1")
        .with_body(thumb.encrypted_data.clone())
        .create_async()
        .await;

    let put: Arc<Mutex<Option<Value>>> = Arc::default();
    let put_seen = Arc::clone(&put);
    let put_mock = server
        .mock("PUT", "/fork/worker-api/files/data")
        .match_header("X-Worker-Access-Token", token)
        .match_request(move |req| {
            *put_seen.lock().unwrap() = serde_json::from_slice(req.body().unwrap()).ok();
            true
        })
        .with_body("{}")
        .create_async()
        .await;
    let consume_mock = server
        .mock("POST", "/fork/worker-api/inbox/consume")
        .match_body(Matcher::Json(json!({"fileIDs": [3, 1]})))
        .with_body(r#"{"consumed": 2}"#)
        .create_async()
        .await;

    let config = Config::parse(&format!(
        "[server]\nendpoint = \"{}\"\npublic_key = \"{worker_pk}\"\n[runtime]\nbatch_size = 8\nconcurrency = 2\n",
        server.url()
    ))
    .unwrap();
    let identity = WorkerIdentity::from_base64(
        b64(worker_sk.as_bytes()).as_bytes(),
        config.public_key.as_deref(),
        Default::default(),
    )
    .unwrap();
    let worker = Worker::new(config, identity).unwrap();
    let summary = worker.run_once().await.unwrap();
    assert_eq!(
        summary,
        Summary {
            users: 1,
            written: 1,
            already_done: 0,
            deferred: 1,
            failed: 0,
            unopenable: 1
        }
    );
    put_mock.assert_async().await;
    consume_mock.assert_async().await;

    let body = put.lock().unwrap().clone().unwrap();
    assert_eq!(body["fileID"], 1);
    assert_eq!(body["type"], "mldata");
    assert_eq!(
        body["lastUpdatedAt"], 555,
        "must send back the timestamp it read"
    );
    let data = STANDARD
        .decode(body["encryptedData"].as_str().unwrap())
        .unwrap();
    let header = ente_core::crypto::Header::try_from_slice(
        &STANDARD
            .decode(body["decryptionHeader"].as_str().unwrap())
            .unwrap(),
    )
    .unwrap();
    let gz = blob::decrypt(&data, &header, &key1).unwrap();
    let mut text = String::new();
    flate2::read::GzDecoder::new(gz.as_slice())
        .read_to_string(&mut text)
        .unwrap();
    // Ente's own keys survive byte for byte, numbers included.
    assert!(
        text.contains(r#""embedding":[0.1234567890123456789,-2.5e-7]"#),
        "{text}"
    );
    let merged: Value = serde_json::from_str(&text).unwrap();
    for k in ["face", "clip", "future"] {
        assert_eq!(merged[k], original[k]);
    }
    let x = &merged["x_enrich"];
    assert_eq!(x["version"], 1);
    assert_eq!(x["device"]["kind"], "screenshot");
    assert_eq!(x["pii"]["kinds"]["iban"], 1);
    assert_eq!(x["pii"]["suggestHidden"], true);
    assert!(!text.contains("GB82"), "PII values are never stored");
    assert_eq!(x["hashes"]["dhash"].as_str().unwrap().len(), 16);
    assert!(x["blur"]["laplacianVariance"].is_number());
}

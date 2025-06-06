use fxhash::FxHashMap;
use loro_common::{
    loro_value, ContainerID, ContainerType, IdSpan, LoroError, LoroResult, LoroValue, PeerID, ID,
};
use loro_internal::awareness::EphemeralStore;
use loro_internal::sync::{AtomicBool, Mutex};
use loro_internal::{
    delta::ResolvedMapValue,
    encoding::ImportStatus,
    event::{Diff, EventTriggerKind},
    fx_map,
    handler::{Handler, TextDelta, ValueOrHandler},
    loro::{CommitOptions, ExportMode},
    version::{Frontiers, VersionRange},
    ApplyDiff, HandlerTrait, ListHandler, LoroDoc, MapHandler, TextHandler, ToJson, TreeHandler,
    TreeParentId,
};
use serde_json::json;
use std::sync::Arc;

#[test]
fn issue_502() -> LoroResult<()> {
    let doc = LoroDoc::new_auto_commit();
    doc.get_map("map").insert("stringA", "Original data")?;
    doc.commit_then_renew();
    doc.get_map("map").insert("stringA", "Updated data")?;
    doc.attach();
    doc.get_map("map").insert("stringB", "Something else")?;
    doc.commit_then_renew();
    Ok(())
}

#[test]
fn issue_225() -> LoroResult<()> {
    let doc = LoroDoc::new_auto_commit();
    let text = doc.get_text("text");
    text.insert(0, "123")?;
    text.mark(0, 3, "bold", true.into())?;
    // when apply_delta, the attributes of insert should override the current styles
    text.apply_delta(&[
        TextDelta::Retain {
            retain: 3,
            attributes: None,
        },
        TextDelta::Insert {
            insert: "new".into(),
            attributes: None,
        },
    ])?;
    assert_eq!(
        text.get_richtext_value().to_json_value(),
        json!([{ "insert": "123", "attributes": { "bold": true } }, { "insert": "new" }])
    );

    Ok(())
}

#[test]
fn issue_211() -> LoroResult<()> {
    let doc1 = LoroDoc::new_auto_commit();
    let doc2 = LoroDoc::new_auto_commit();
    doc1.get_text("text").insert(0, "T")?;
    doc2.merge(&doc1)?;
    let v0 = doc1.oplog_frontiers();
    doc1.get_text("text").insert(1, "A")?;
    doc2.get_text("text").insert(1, "B")?;
    doc1.checkout(&v0)?;
    doc2.checkout(&v0)?;
    doc1.checkout_to_latest();
    doc2.checkout_to_latest();
    // let v1_of_doc1 = doc1.oplog_frontiers();
    let v1_of_doc2 = doc2.oplog_frontiers();
    doc2.get_text("text").insert(2, "B")?;
    doc2.checkout(&v1_of_doc2)?;
    doc2.checkout(&v0)?;
    assert_eq!(
        doc2.get_deep_value().to_json_value(),
        json!({
            "text": "T"
        })
    );
    Ok(())
}

#[test]
fn mark_with_the_same_key_value_should_be_skipped() {
    let a = LoroDoc::new_auto_commit();
    let text = a.get_text("text");
    text.insert(0, "Hello world!").unwrap();
    text.mark(0, 11, "bold", "value".into()).unwrap();
    a.commit_then_renew();
    let v = a.oplog_vv();
    text.mark(0, 5, "bold", "value".into()).unwrap();
    a.commit_then_renew();
    let new_v = a.oplog_vv();
    // new mark should be ignored, so vv should be the same
    assert_eq!(v, new_v);
}

#[test]
fn event_from_checkout() {
    let a = LoroDoc::new_auto_commit();
    let sub = a.subscribe_root(Arc::new(|event| {
        assert!(matches!(
            event.event_meta.by,
            EventTriggerKind::Checkout | EventTriggerKind::Local
        ));
    }));
    a.get_text("text").insert(0, "hello").unwrap();
    a.commit_then_renew();
    let version = a.oplog_frontiers();
    a.get_text("text").insert(0, "hello").unwrap();
    a.commit_then_renew();
    sub.unsubscribe();
    let ran = Arc::new(AtomicBool::new(false));
    let ran_cloned = ran.clone();
    let _g = a.subscribe_root(Arc::new(move |event| {
        assert!(event.event_meta.by.is_checkout());
        ran.store(true, std::sync::atomic::Ordering::Relaxed);
    }));
    a.checkout(&version).unwrap();
    assert!(ran_cloned.load(std::sync::atomic::Ordering::Relaxed));
}

#[test]
fn handler_in_event() {
    let doc = LoroDoc::new_auto_commit();
    let _g = doc.subscribe_root(Arc::new(|e| {
        dbg!(&e);
        let value = e.events[0]
            .diff
            .as_list()
            .unwrap()
            .iter()
            .next()
            .unwrap()
            .as_replace()
            .unwrap()
            .0
            .iter()
            .next()
            .unwrap();
        assert!(matches!(value, ValueOrHandler::Handler(Handler::Text(_))));
    }));
    let list = doc.get_list("list");
    list.insert_container(0, TextHandler::new_detached())
        .unwrap();
    doc.commit_then_renew();
}

#[test]
fn out_of_bound_test() {
    let a = LoroDoc::new_auto_commit();
    a.get_text("text").insert(0, "Hello").unwrap();
    a.get_list("list").insert(0, "Hello").unwrap();
    a.get_list("list").insert(1, "Hello").unwrap();
    // expect out of bound err
    let err = a.get_text("text").insert(6, "Hello").unwrap_err();
    assert!(matches!(err, loro_common::LoroError::OutOfBound { .. }));
    let err = a.get_text("text").delete(3, 5).unwrap_err();
    assert!(matches!(err, loro_common::LoroError::OutOfBound { .. }));
    let err = a.get_text("text").mark(0, 8, "h", 5.into()).unwrap_err();
    assert!(matches!(err, loro_common::LoroError::OutOfBound { .. }));
    let _err = a.get_text("text").mark(3, 0, "h", 5.into()).unwrap_err();
    let err = a.get_list("list").insert(6, "Hello").unwrap_err();
    assert!(matches!(err, loro_common::LoroError::OutOfBound { .. }));
    let err = a.get_list("list").delete(3, 2).unwrap_err();
    assert!(matches!(err, loro_common::LoroError::OutOfBound { .. }));
    let err = a
        .get_list("list")
        .insert_container(3, MapHandler::new_detached())
        .unwrap_err();
    assert!(matches!(err, loro_common::LoroError::OutOfBound { .. }));
}

#[test]
fn list() {
    let a = LoroDoc::new_auto_commit();
    a.get_list("list").insert(0, "Hello").unwrap();
    assert_eq!(a.get_list("list").get(0).unwrap(), LoroValue::from("Hello"));
    let map = a
        .get_list("list")
        .insert_container(1, MapHandler::new_detached())
        .unwrap();
    map.insert("Hello", LoroValue::from("u")).unwrap();
    let pos = map
        .insert_container("pos", MapHandler::new_detached())
        .unwrap();
    pos.insert("x", 0).unwrap();
    pos.insert("y", 100).unwrap();

    let cid = map.id();
    let id = a.get_list("list").get(1);
    assert_eq!(id.as_ref().unwrap().as_container().unwrap(), &cid);
    let map = a.get_map(id.unwrap().into_container().unwrap());
    let new_pos = a.get_map(map.get("pos").unwrap().into_container().unwrap());
    assert_eq!(
        new_pos.get_deep_value().to_json_value(),
        json!({
            "x": 0,
            "y": 100,
        })
    );
}

#[test]
fn richtext_mark_event() {
    let a = LoroDoc::new_auto_commit();
    let _g = a.subscribe(
        &a.get_text("text").id(),
        Arc::new(|e| {
            let delta = e.events[0].diff.as_text().unwrap();
            assert_eq!(
                delta.to_json_value(),
                json!([
                        {"insert": "He", "attributes": {"bold": true}},
                        {"insert": "ll", "attributes": {"bold": null}},
                        {"insert": "o", "attributes": {"bold": true}}
                ])
            )
        }),
    );
    a.get_text("text").insert(0, "Hello").unwrap();
    a.get_text("text").mark(0, 5, "bold", true.into()).unwrap();
    a.get_text("text")
        .mark(2, 4, "bold", LoroValue::Null)
        .unwrap();
    let _ = a.commit_then_stop();
    let b = LoroDoc::new_auto_commit();
    let _g = b.subscribe(
        &a.get_text("text").id(),
        Arc::new(|e| {
            let delta = e.events[0].diff.as_text().unwrap();
            assert_eq!(
                delta.to_json_value(),
                json!([
                    {"insert": "He", "attributes": {"bold": true}},
                    {"insert": "ll", "attributes": {"bold": null}},
                    {"insert": "o", "attributes": {"bold": true}}
                ])
            )
        }),
    );
    b.merge(&a).unwrap();
}

#[test]
fn concurrent_richtext_mark_event() {
    let a = LoroDoc::new_auto_commit();
    let b = LoroDoc::new_auto_commit();
    let c = LoroDoc::new_auto_commit();
    a.get_text("text").insert(0, "Hello").unwrap();
    b.merge(&a).unwrap();
    c.merge(&a).unwrap();
    b.get_text("text").mark(0, 3, "bold", true.into()).unwrap();
    c.get_text("text").mark(1, 4, "link", true.into()).unwrap();
    b.merge(&c).unwrap();
    let sub_id = a.subscribe(
        &a.get_text("text").id(),
        Arc::new(|e| {
            let delta = e.events[0].diff.as_text().unwrap();
            assert_eq!(
                delta.to_json_value(),
                json!([
                    {"retain": 1, "attributes": {"bold": true, }},
                    {"retain": 2, "attributes": {"bold": true, "link": true}},
                    {"retain": 1, "attributes": {"link": true}},
                ])
            )
        }),
    );

    a.merge(&b).unwrap();
    sub_id.unsubscribe();

    let sub_id = a.subscribe(
        &a.get_text("text").id(),
        Arc::new(|e| {
            let delta = e.events[0].diff.as_text().unwrap();
            assert_eq!(
                delta.to_json_value(),
                json!([
                    {
                        "retain": 2,
                    },
                    {
                        "retain": 1,
                        "attributes": {"bold": null, "link": true}
                    }
                ])
            )
        }),
    );

    b.get_text("text")
        .mark(2, 3, "bold", LoroValue::Null)
        .unwrap();
    a.merge(&b).unwrap();
    sub_id.unsubscribe();
    let _g = a.subscribe(
        &a.get_text("text").id(),
        Arc::new(|e| {
            for container_diff in e.events {
                let delta = container_diff.diff.as_text().unwrap();
                assert_eq!(
                    delta.to_json_value(),
                    json!([
                        {
                            "retain": 2,
                        },
                        {
                            "insert": "A",
                            "attributes": {"bold": true, "link": true}
                        }
                    ])
                )
            }
        }),
    );
    a.get_text("text").insert(2, "A").unwrap();
    let _ = a.commit_then_stop();
}

#[test]
fn insert_richtext_event() {
    let a = LoroDoc::new_auto_commit();
    a.get_text("text").insert(0, "Hello").unwrap();
    a.get_text("text").mark(0, 5, "bold", true.into()).unwrap();
    a.commit_then_renew();
    let text = a.get_text("text");
    let _g = a.subscribe(
        &text.id(),
        Arc::new(|e| {
            let delta = e.events[0].diff.as_text().unwrap();
            assert_eq!(
                delta.to_json_value(),
                json!([
                        {"retain": 5,},
                        {"insert": " World!", "attributes": {"bold": true}}
                ])
            )
        }),
    );

    text.insert(5, " World!").unwrap();
}

#[test]
fn import_after_init_handlers() {
    let a = LoroDoc::new_auto_commit();
    let _g = a.subscribe(
        &ContainerID::new_root("text", ContainerType::Text),
        Arc::new(|event| {
            assert!(matches!(
                event.events[0].diff,
                loro_internal::event::Diff::Text(_)
            ))
        }),
    );
    let _g = a.subscribe(
        &ContainerID::new_root("map", ContainerType::Map),
        Arc::new(|event| {
            assert!(matches!(
                event.events[0].diff,
                loro_internal::event::Diff::Map(_)
            ))
        }),
    );
    let _g = a.subscribe(
        &ContainerID::new_root("list", ContainerType::List),
        Arc::new(|event| {
            assert!(matches!(
                event.events[0].diff,
                loro_internal::event::Diff::List(_)
            ))
        }),
    );

    let b = LoroDoc::new_auto_commit();
    b.get_list("list").insert(0, "list").unwrap();
    b.get_list("list_a").insert(0, "list_a").unwrap();
    b.get_text("text").insert(0, "text").unwrap();
    b.get_map("map").insert("m", "map").unwrap();
    a.import(&b.export_snapshot().unwrap()).unwrap();
    a.commit_then_renew();
}

#[test]
fn test_from_snapshot() {
    let a = LoroDoc::new_auto_commit();
    a.get_text("text").insert(0, "0").unwrap();
    let snapshot = a.export_snapshot().unwrap();
    let c = LoroDoc::from_snapshot(&snapshot).unwrap();
    assert_eq!(a.get_deep_value(), c.get_deep_value());
    assert_eq!(a.oplog_frontiers(), c.oplog_frontiers());
    assert_eq!(a.state_frontiers(), c.state_frontiers());
    let updates = a.export_from(&Default::default());
    let d = match LoroDoc::from_snapshot(&updates) {
        Ok(_) => panic!(),
        Err(e) => e,
    };
    assert!(matches!(d, loro_common::LoroError::DecodeError(..)));
}

#[test]
fn test_pending() {
    let a = LoroDoc::new_auto_commit();
    a.set_peer_id(0).unwrap();
    a.get_text("text").insert(0, "0").unwrap();
    let b = LoroDoc::new_auto_commit();
    b.set_peer_id(1).unwrap();
    b.import(&a.export_from(&Default::default())).unwrap();
    b.get_text("text").insert(0, "1").unwrap();
    let c = LoroDoc::new_auto_commit();
    b.set_peer_id(2).unwrap();
    c.import(&b.export_from(&Default::default())).unwrap();
    c.get_text("text").insert(0, "2").unwrap();

    // c creates a pending change for a, insert "2" cannot be merged into a yet
    a.import(&c.export_from(&b.oplog_vv())).unwrap();
    assert_eq!(a.get_deep_value().to_json_value(), json!({"text": "0"}));

    // b does not has c's change
    a.import(&b.export_from(&a.oplog_vv())).unwrap();
    dbg!(&a.oplog().lock().unwrap());
    assert_eq!(a.get_deep_value().to_json_value(), json!({"text": "210"}));
}

#[test]
fn test_checkout() {
    let doc_0 = LoroDoc::new_auto_commit();
    doc_0.set_peer_id(0).unwrap();
    let doc_1 = LoroDoc::new_auto_commit();
    doc_1.set_peer_id(1).unwrap();

    let value: Arc<Mutex<LoroValue>> = Arc::new(Mutex::new(LoroValue::Map(Default::default())));
    let root_value = value.clone();
    let _g = doc_0.subscribe_root(Arc::new(move |event| {
        dbg!(&event);
        let mut root_value = root_value.lock().unwrap();
        for container_diff in event.events {
            root_value.apply(
                &container_diff.path.iter().map(|x| x.1.clone()).collect(),
                &[container_diff.diff.clone()],
            );
        }
    }));

    let map = doc_0.get_map("map");
    let handler = map
        .insert_container("text", TextHandler::new_detached())
        .unwrap();
    let text = handler;
    text.insert(0, "123").unwrap();

    let map = doc_1.get_map("map");

    let handler = map
        .insert_container("text", TextHandler::new_detached())
        .unwrap();
    let text = handler;
    text.insert(0, "123").unwrap();

    doc_0
        .import(&doc_1.export_from(&Default::default()))
        .unwrap();

    doc_0
        .checkout(&Frontiers::from(vec![ID::new(0, 2)]))
        .unwrap();

    assert_eq!(&doc_0.get_deep_value(), &*value.lock().unwrap());
    assert_eq!(
        value.lock().unwrap().to_json_value(),
        json!({
            "map": {
                "text": "12"
            }
        })
    );
}

#[test]
fn test_timestamp() {
    let doc = LoroDoc::new();
    doc.set_record_timestamp(true);
    let text = doc.get_text("text");
    let mut txn = doc.txn().unwrap();
    text.insert_with_txn(&mut txn, 0, "123").unwrap();
    txn.commit().unwrap();
    let op_log = &doc.oplog().lock().unwrap();
    let change = op_log.get_change_at(ID::new(doc.peer_id(), 0)).unwrap();
    assert!(change.timestamp() > 1690966970);
}

#[test]
fn test_text_checkout() {
    let doc = LoroDoc::new_auto_commit();
    doc.set_peer_id(1).unwrap();
    let text = doc.get_text("text");
    text.insert(0, "你界").unwrap();
    text.insert(1, "好世").unwrap();
    doc.commit_then_renew();
    {
        doc.checkout(&Frontiers::from([ID::new(doc.peer_id(), 0)].as_slice()))
            .unwrap();
        assert_eq!(text.get_value().as_string().unwrap().as_str(), "你");
    }
    {
        doc.checkout(&Frontiers::from([ID::new(doc.peer_id(), 1)].as_slice()))
            .unwrap();
        assert_eq!(text.get_value().as_string().unwrap().as_str(), "你界");
    }
    {
        doc.checkout(&Frontiers::from([ID::new(doc.peer_id(), 2)].as_slice()))
            .unwrap();
        assert_eq!(text.get_value().as_string().unwrap().as_str(), "你好界");
    }
    {
        doc.checkout(&Frontiers::from([ID::new(doc.peer_id(), 3)].as_slice()))
            .unwrap();
        assert_eq!(text.get_value().as_string().unwrap().as_str(), "你好世界");
    }
    assert_eq!(text.len_unicode(), 4);
    assert_eq!(text.len_utf8(), 12);
    assert_eq!(text.len_unicode(), 4);

    doc.checkout_to_latest();
    text.delete(3, 1).unwrap();
    assert_eq!(text.get_value().as_string().unwrap().as_str(), "你好世");
    text.delete(2, 1).unwrap();
    assert_eq!(text.get_value().as_string().unwrap().as_str(), "你好");
    doc.checkout(&Frontiers::from([ID::new(doc.peer_id(), 3)].as_slice()))
        .unwrap();
    assert_eq!(text.get_value().as_string().unwrap().as_str(), "你好世界");
    doc.checkout(&Frontiers::from([ID::new(doc.peer_id(), 4)].as_slice()))
        .unwrap();
    assert_eq!(text.get_value().as_string().unwrap().as_str(), "你好世");
    doc.checkout(&Frontiers::from([ID::new(doc.peer_id(), 5)].as_slice()))
        .unwrap();
    assert_eq!(text.get_value().as_string().unwrap().as_str(), "你好");
    {
        doc.checkout(&Frontiers::from([ID::new(doc.peer_id(), 0)].as_slice()))
            .unwrap();
        assert_eq!(text.get_value().as_string().unwrap().as_str(), "你");
    }
    {
        doc.checkout(&Frontiers::from([ID::new(doc.peer_id(), 1)].as_slice()))
            .unwrap();
        assert_eq!(text.get_value().as_string().unwrap().as_str(), "你界");
    }
    {
        doc.checkout(&Frontiers::from([ID::new(doc.peer_id(), 2)].as_slice()))
            .unwrap();
        assert_eq!(text.get_value().as_string().unwrap().as_str(), "你好界");
    }
    {
        doc.checkout(&Frontiers::from([ID::new(doc.peer_id(), 3)].as_slice()))
            .unwrap();
        assert_eq!(text.get_value().as_string().unwrap().as_str(), "你好世界");
    }
}

#[test]
fn map_checkout() {
    let doc = LoroDoc::new_auto_commit();
    let meta = doc.get_map("meta");
    let v_empty = doc.oplog_frontiers();
    meta.insert("key", 0).unwrap();
    let v0 = doc.oplog_frontiers();
    meta.insert("key", 1).unwrap();
    let v1 = doc.oplog_frontiers();
    assert_eq!(meta.get_deep_value().to_json(), r#"{"key":1}"#);
    doc.checkout(&v0).unwrap();
    assert_eq!(meta.get_deep_value().to_json(), r#"{"key":0}"#);
    doc.checkout(&v_empty).unwrap();
    assert_eq!(meta.get_deep_value().to_json(), r#"{}"#);
    doc.checkout(&v1).unwrap();
    assert_eq!(meta.get_deep_value().to_json(), r#"{"key":1}"#);
}

#[test]
fn a_list_of_map_checkout() {
    let doc = LoroDoc::new_auto_commit();
    let entry = doc.get_map("entry");
    let (list, sub) = {
        let list = entry
            .insert_container("list", ListHandler::new_detached())
            .unwrap();
        let sub_map = list
            .insert_container(0, MapHandler::new_detached())
            .unwrap();
        sub_map.insert("x", 100).unwrap();
        sub_map.insert("y", 1000).unwrap();
        (list, sub_map)
    };
    let v0 = doc.oplog_frontiers();
    let d0 = doc.get_deep_value().to_json();

    list.insert(0, 3).unwrap();
    list.push(4).unwrap();
    list.insert_container(2, MapHandler::new_detached())
        .unwrap();
    list.insert_container(3, TextHandler::new_detached())
        .unwrap();

    list.delete(2, 1).unwrap();
    sub.insert("x", 9).unwrap();
    sub.insert("y", 9).unwrap();
    sub.insert("z", 9).unwrap();
    let v1 = doc.oplog_frontiers();
    let d1 = doc.get_deep_value().to_json();
    sub.insert("x", 77).unwrap();
    sub.insert("y", 88).unwrap();
    list.delete(0, 1).unwrap();
    list.insert(0, 123).unwrap();
    list.push(99).unwrap();
    let v2 = doc.oplog_frontiers();
    let d2 = doc.get_deep_value().to_json();

    doc.checkout(&v0).unwrap();
    assert_eq!(doc.get_deep_value().to_json(), d0);
    doc.checkout(&v1).unwrap();
    assert_eq!(doc.get_deep_value().to_json(), d1);
    doc.checkout(&v2).unwrap();
    println!("{}", doc.get_deep_value_with_id().to_json_pretty());
    assert_eq!(doc.get_deep_value().to_json(), d2);
    doc.checkout(&v1).unwrap();

    println!("{}", doc.get_deep_value_with_id().to_json_pretty());
    assert_eq!(doc.get_deep_value().to_json(), d1);
    doc.checkout(&v0).unwrap();
    assert_eq!(doc.get_deep_value().to_json(), d0);
}

#[test]
fn map_concurrent_checkout() {
    let doc_a = LoroDoc::new_auto_commit();
    let meta_a = doc_a.get_map("meta");
    let doc_b = LoroDoc::new_auto_commit();
    let meta_b = doc_b.get_map("meta");

    meta_a.insert("key", 0).unwrap();
    let va = doc_a.oplog_frontiers();
    meta_b.insert("s", 1).unwrap();
    let vb_0 = doc_b.oplog_frontiers();
    meta_b.insert("key", 1).unwrap();
    let vb_1 = doc_b.oplog_frontiers();
    doc_a.import(&doc_b.export_snapshot().unwrap()).unwrap();
    meta_a.insert("key", 2).unwrap();

    let v_merged = doc_a.oplog_frontiers();

    doc_a.checkout(&va).unwrap();
    assert_eq!(meta_a.get_deep_value().to_json(), r#"{"key":0}"#);
    doc_a.checkout(&vb_0).unwrap();
    assert_eq!(meta_a.get_deep_value().to_json(), r#"{"s":1}"#);
    doc_a.checkout(&vb_1).unwrap();
    assert_eq!(meta_a.get_deep_value().to_json(), r#"{"s":1,"key":1}"#);
    doc_a.checkout(&v_merged).unwrap();
    assert_eq!(meta_a.get_deep_value().to_json(), r#"{"s":1,"key":2}"#);
}

#[test]
fn tree_checkout() {
    let doc_a = LoroDoc::new_auto_commit();
    let _g = doc_a.subscribe_root(Arc::new(|_e| {}));
    doc_a.set_peer_id(1).unwrap();
    let tree = doc_a.get_tree("root");
    let id1 = tree.create(TreeParentId::Root).unwrap();
    let id2 = tree.create(TreeParentId::Node(id1)).unwrap();
    let v1_state = tree.get_deep_value();
    let v1 = doc_a.oplog_frontiers();
    let _id3 = tree.create(TreeParentId::Node(id2)).unwrap();
    let v2_state = tree.get_deep_value();
    let v2 = doc_a.oplog_frontiers();
    tree.delete(id2).unwrap();
    let v3_state = tree.get_deep_value();
    let v3 = doc_a.oplog_frontiers();
    doc_a.checkout(&v1).unwrap();
    assert_eq!(
        serde_json::to_value(tree.get_deep_value())
            .unwrap()
            .get("roots"),
        serde_json::to_value(v1_state).unwrap().get("roots")
    );
    doc_a.checkout(&v2).unwrap();
    assert_eq!(
        serde_json::to_value(tree.get_deep_value())
            .unwrap()
            .get("roots"),
        serde_json::to_value(v2_state).unwrap().get("roots")
    );
    doc_a.checkout(&v3).unwrap();
    assert_eq!(
        serde_json::to_value(tree.get_deep_value())
            .unwrap()
            .get("roots"),
        serde_json::to_value(v3_state).unwrap().get("roots")
    );

    doc_a.attach();
    tree.create(TreeParentId::Root).unwrap();
}

#[test]
fn issue_batch_import_snapshot() {
    let doc = LoroDoc::new_auto_commit();
    doc.set_peer_id(123).unwrap();
    let doc2 = LoroDoc::new_auto_commit();
    doc2.set_peer_id(456).unwrap();
    doc.get_map("map").insert("s", "hello world!").unwrap();
    doc2.get_map("map").insert("s", "hello?").unwrap();

    let data1 = doc.export_snapshot().unwrap();
    let data2 = doc2.export_snapshot().unwrap();
    let doc3 = LoroDoc::new();
    doc3.import_batch(&[data1, data2]).unwrap();
}

#[test]
fn state_may_deadlock_when_import() {
    // helper function ref: https://github.com/rust-lang/rfcs/issues/2798#issuecomment-552949300
    use std::time::Duration;
    use std::{sync::mpsc, thread};
    fn panic_after<T, F>(d: Duration, f: F) -> T
    where
        T: Send + 'static,
        F: FnOnce() -> T,
        F: Send + 'static,
    {
        let (done_tx, done_rx) = mpsc::channel();
        let handle = thread::spawn(move || {
            let val = f();
            done_tx.send(()).expect("Unable to send completion signal");
            val
        });

        match done_rx.recv_timeout(d) {
            Ok(_) => handle.join().expect("Thread panicked"),
            Err(_) => panic!("Thread took too long"),
        }
    }

    panic_after(Duration::from_millis(100), || {
        let doc = LoroDoc::new_auto_commit();
        let map = doc.get_map("map");
        let _g = doc.subscribe_root(Arc::new(move |_e| {
            map.id();
        }));

        let doc2 = LoroDoc::new_auto_commit();
        doc2.get_map("map").insert("foo", 123).unwrap();
        doc.import(&doc.export_snapshot().unwrap()).unwrap();
    })
}

#[ctor::ctor]
fn init() {
    dev_utils::setup_test_log();
}

#[test]
fn missing_event_when_checkout() {
    let doc = LoroDoc::new_auto_commit();
    doc.checkout(&doc.oplog_frontiers()).unwrap();
    let value = Arc::new(Mutex::new(FxHashMap::default()));
    let map = value.clone();
    let _g = doc.subscribe(
        &ContainerID::new_root("tree", ContainerType::Tree),
        Arc::new(move |e| {
            let mut v = map.lock().unwrap();
            for container_diff in e.events.iter() {
                let from_children =
                    container_diff.id != ContainerID::new_root("tree", ContainerType::Tree);
                if from_children {
                    if let Diff::Map(map) = &container_diff.diff {
                        for (k, ResolvedMapValue { value, .. }) in map.updated.iter() {
                            match value {
                                Some(value) => {
                                    v.insert(
                                        k.to_string(),
                                        *value.as_value().unwrap().as_i64().unwrap(),
                                    );
                                }
                                None => {
                                    v.remove(&k.to_string());
                                }
                            }
                        }
                    }
                }
            }
        }),
    );

    let doc2 = LoroDoc::new_auto_commit();
    let tree = doc2.get_tree("tree");
    let node = tree.create_at(TreeParentId::Root, 0).unwrap();
    let _ = tree.create_at(TreeParentId::Root, 0).unwrap();
    let meta = tree.get_meta(node).unwrap();
    meta.insert("a", 0).unwrap();
    doc.import(&doc2.export_from(&doc.oplog_vv())).unwrap();
    doc.attach();
    meta.insert("b", 1).unwrap();
    doc.checkout(&doc.oplog_frontiers()).unwrap();
    doc.import(&doc2.export_from(&doc.oplog_vv())).unwrap();
    // checkout use the same diff_calculator, the depth of calculator is not updated
    doc.attach();
    assert!(value.lock().unwrap().contains_key("b"));
}

#[test]
fn empty_event() {
    let doc = LoroDoc::new_auto_commit();
    doc.get_map("map").insert("key", 123).unwrap();
    doc.commit_then_renew();
    let fire = Arc::new(AtomicBool::new(false));
    let fire_clone = Arc::clone(&fire);
    let _g = doc.subscribe_root(Arc::new(move |_e| {
        fire_clone.store(true, std::sync::atomic::Ordering::Relaxed);
    }));
    doc.import(&doc.export_snapshot().unwrap()).unwrap();
    assert!(!fire.load(std::sync::atomic::Ordering::Relaxed));
}

#[test]
fn insert_attach_container() -> LoroResult<()> {
    let doc = LoroDoc::new_auto_commit();
    let list = doc.get_list("list");
    list.insert_container(0, MapHandler::new_detached())?
        .insert("key", 1)?;
    list.insert_container(1, MapHandler::new_detached())?
        .insert("key", 2)?;
    let elem = list.insert_container(2, MapHandler::new_detached())?;
    elem.insert("key", 3)?;
    let new_map = list.insert_container(0, elem)?;
    assert_eq!(new_map.get_value().to_json_value(), json!({"key": 3}));
    list.delete(3, 1)?;

    let elem = list.insert_container(0, TextHandler::new_detached())?;
    elem.insert(0, "abc")?;
    elem.mark(0, 2, "bold", true.into())?;
    let new_text = list.insert_container(0, elem)?;
    assert_eq!(
        new_text.get_richtext_value().to_json_value(),
        json!([{"insert":"ab", "attributes": {"bold": true}}, {"insert":"c"}])
    );

    let elem = list.insert_container(0, ListHandler::new_detached())?;
    elem.insert(0, "list")?;
    let new_list = list.insert_container(0, elem)?;
    new_list.insert(0, "new_list")?;
    assert_eq!(
        new_list.get_value().to_json_value(),
        json!(["new_list", "list"])
    );

    // let elem = list.insert_container(2, TreeHandler::new_detached())?;
    // let p = elem.create(None)?;
    // elem.create(p)?;
    // list.insert_container(0, elem)?;

    assert_eq!(
        doc.get_deep_value().to_json_value(),
        json!({
            "list": [["new_list", "list"], ["list"],"abc", "abc", {"key": 3}, {"key": 1}, {"key": 2}]
        })
    );
    Ok(())
}

#[test]
fn tree_attach() {
    let tree = TreeHandler::new_detached();
    let id = tree.create(TreeParentId::Root).unwrap();
    tree.get_meta(id).unwrap().insert("key", "value").unwrap();
    let doc = LoroDoc::new_auto_commit();
    doc.get_list("list").insert_container(0, tree).unwrap();
    let v = doc.get_deep_value();
    assert_eq!(
        v.as_map().unwrap().get("list").unwrap().as_list().unwrap()[0]
            .as_list()
            .unwrap()[0]
            .as_map()
            .unwrap()
            .get("meta")
            .unwrap()
            .to_json_value(),
        json!({"key":"value"})
    )
}

#[test]
#[cfg(feature = "counter")]
fn counter() {
    let doc = LoroDoc::new_auto_commit();
    let counter = doc.get_counter("counter");
    counter.increment(1.).unwrap();
    counter.increment(2.).unwrap();
    counter.decrement(1.).unwrap();
    let json = doc.export_json_updates(&Default::default(), &doc.oplog_vv(), true);
    let doc2 = LoroDoc::new_auto_commit();
    doc2.import_json_updates(json).unwrap();
}

#[test]
fn test_insert_utf8() {
    let doc = LoroDoc::new_auto_commit();
    let text = doc.get_text("text");
    text.insert_utf8(0, "Hello ").unwrap();
    text.insert_utf8(6, "World").unwrap();
    assert_eq!(
        text.get_richtext_value().to_json_value(),
        json!([{"insert":"Hello World"}])
    )
}

#[test]
fn test_insert_utf8_cross_unicode_1() {
    let doc = LoroDoc::new_auto_commit();
    let text = doc.get_text("text");
    text.insert_utf8(0, "你好").unwrap();
    text.insert_utf8(3, "World").unwrap();
    assert_eq!(
        text.get_richtext_value().to_json_value(),
        json!([{"insert":"你World好"}])
    )
}

#[test]
fn test_insert_utf8_cross_unicode_2() {
    let doc = LoroDoc::new_auto_commit();
    let text = doc.get_text("text");
    text.insert_utf8(0, "你好").unwrap();
    text.insert_utf8(6, "World").unwrap();
    assert_eq!(
        text.get_richtext_value().to_json_value(),
        json!([{"insert":"你好World"}])
    )
}

#[test]
fn test_insert_utf8_detached() {
    let text = TextHandler::new_detached();
    text.insert_utf8(0, "Hello ").unwrap();
    text.insert_utf8(6, "World").unwrap();
    assert_eq!(
        text.get_richtext_value().to_json_value(),
        json!([{"insert":"Hello World"}])
    )
}

#[test]
#[should_panic]
#[ignore = "fix me later after gbtree support Result for query"]
fn test_insert_utf8_panic_cross_unicode() {
    let doc = LoroDoc::new_auto_commit();
    let text = doc.get_text("text");
    text.insert_utf8(0, "你好").unwrap();
    text.insert_utf8(1, "World").unwrap();
}

#[test]
#[should_panic]
fn test_insert_utf8_panic_out_bound() {
    let doc = LoroDoc::new_auto_commit();
    let text = doc.get_text("text");
    text.insert_utf8(0, "Hello ").unwrap();
    text.insert_utf8(7, "World").unwrap();
}

//    println!("{}", text.get_richtext_value().to_json_value().to_string());

#[test]
fn test_delete_utf8() {
    let doc = LoroDoc::new_auto_commit();
    let text = doc.get_text("text");
    text.insert_utf8(0, "Hello").unwrap();
    text.delete_utf8(1, 3).unwrap();
    assert_eq!(
        text.get_richtext_value().to_json_value(),
        json!([{"insert":"Ho"}])
    )
}

#[test]
fn test_delete_utf8_with_zero_len() {
    let doc = LoroDoc::new_auto_commit();
    let text = doc.get_text("text");
    text.insert_utf8(0, "Hello").unwrap();
    text.delete_utf8(1, 0).unwrap();
    assert_eq!(
        text.get_richtext_value().to_json_value(),
        json!([{"insert":"Hello"}])
    )
}

#[test]
fn test_delete_utf8_cross_unicode() {
    let doc = LoroDoc::new_auto_commit();
    let text = doc.get_text("text");
    text.insert_utf8(0, "你好").unwrap();
    text.delete_utf8(0, 3).unwrap();
    assert_eq!(
        text.get_richtext_value().to_json_value(),
        json!([{"insert":"好"}])
    )
}

#[test]
fn test_delete_utf8_detached() {
    let text = TextHandler::new_detached();
    text.insert_utf8(0, "Hello").unwrap();
    text.delete_utf8(1, 3).unwrap();
    assert_eq!(
        text.get_richtext_value().to_json_value(),
        json!([{"insert":"Ho"}])
    )
}

// WARNING:
// Due to the current inability to report an error on
// get_offset_and_found on BTree, this test won't be ok.
// #[test]
// #[should_panic]
// fn test_delete_utf8_panic_cross_unicode() {
//     let doc = LoroDoc::new_auto_commit();
//     let text = doc.get_text("text");
//     text.insert_utf8(0, "你好").unwrap();
//     text.delete_utf8(0, 2).unwrap();
// }

#[test]
#[should_panic]
fn test_delete_utf8_panic_out_bound_pos() {
    let doc = LoroDoc::new_auto_commit();
    let text = doc.get_text("text");
    text.insert(0, "Hello").unwrap();
    text.delete_utf8(10, 1).unwrap();
}

#[test]
#[should_panic]
fn test_delete_utf8_panic_out_bound_len() {
    let doc = LoroDoc::new_auto_commit();
    let text = doc.get_text("text");
    text.insert(0, "Hello").unwrap();
    text.delete_utf8(1, 10).unwrap();
}

#[test]
fn test_char_at() {
    let doc = LoroDoc::new_auto_commit();
    let text = doc.get_text("text");
    text.insert(0, "Herld").unwrap();
    text.insert(2, "llo Wo").unwrap();
    assert_eq!(text.char_at(0).unwrap(), 'H');
    assert_eq!(text.char_at(1).unwrap(), 'e');
    assert_eq!(text.char_at(2).unwrap(), 'l');
    assert_eq!(text.char_at(3).unwrap(), 'l');
    let err = text.char_at(15).unwrap_err();
    assert!(matches!(err, loro_common::LoroError::OutOfBound { .. }))
}

#[test]
fn test_char_at_detached() {
    let text = TextHandler::new_detached();
    text.insert(0, "Herld").unwrap();
    text.insert(2, "llo Wo").unwrap();
    assert_eq!(text.char_at(0).unwrap(), 'H');
    assert_eq!(text.char_at(1).unwrap(), 'e');
    assert_eq!(text.char_at(2).unwrap(), 'l');
    assert_eq!(text.char_at(3).unwrap(), 'l');
    let err = text.char_at(15).unwrap_err();
    assert!(matches!(err, loro_common::LoroError::OutOfBound { .. }))
}

#[test]
fn test_char_at_wchar() {
    let doc = LoroDoc::new_auto_commit();
    let text = doc.get_text("text");
    text.insert(0, "你好").unwrap();
    text.insert(1, "世界").unwrap();
    assert_eq!(text.char_at(0).unwrap(), '你');
    assert_eq!(text.char_at(1).unwrap(), '世');
    assert_eq!(text.char_at(2).unwrap(), '界');
    assert_eq!(text.char_at(3).unwrap(), '好');
    let err = text.char_at(5).unwrap_err();
    assert!(matches!(err, loro_common::LoroError::OutOfBound { .. }))
}

#[test]
fn test_text_slice() {
    let doc = LoroDoc::new_auto_commit();
    let text = doc.get_text("text");
    text.insert(0, "Hello").unwrap();
    text.insert(1, "World").unwrap();
    assert_eq!(text.slice(0, 4).unwrap(), "HWor");
    assert_eq!(text.slice(0, 1).unwrap(), "H");
}

#[test]
fn test_text_slice_detached() {
    let text = TextHandler::new_detached();
    text.insert(0, "Herld").unwrap();
    text.insert(2, "llo Wo").unwrap();
    assert_eq!(text.slice(0, 4).unwrap(), "Hell");
    assert_eq!(text.slice(0, 1).unwrap(), "H");
}

#[test]
fn test_text_slice_wchar() {
    let doc = LoroDoc::new_auto_commit();
    let text = doc.get_text("text");
    text.insert(0, "你好").unwrap();
    text.insert(1, "世界").unwrap();
    assert_eq!(text.slice(0, 3).unwrap(), "你世界");
}

#[test]
#[should_panic]
fn test_text_slice_end_index_less_than_start() {
    let doc = LoroDoc::new_auto_commit();
    let text = doc.get_text("text");
    text.insert(0, "你好").unwrap();
    text.insert(1, "世界").unwrap();
    text.slice(2, 1).unwrap();
}

#[test]
#[should_panic]
fn test_text_slice_out_of_bound() {
    let doc = LoroDoc::new_auto_commit();
    let text = doc.get_text("text");
    text.insert(0, "你好").unwrap();
    text.insert(1, "世界").unwrap();
    text.slice(1, 10).unwrap();
}

#[test]
fn test_text_splice() {
    let doc = LoroDoc::new_auto_commit();
    let text = doc.get_text("text");
    text.insert(0, "你好").unwrap();
    assert_eq!(text.splice(1, 1, "世界").unwrap(), "好");
    assert_eq!(text.to_string(), "你世界");
}

#[test]
fn test_text_iter() {
    let mut str = String::new();
    let doc = LoroDoc::new_auto_commit();
    let text = doc.get_text("text");
    text.insert(0, "Hello").unwrap();
    text.insert(1, "Hello").unwrap();
    text.iter(|s| {
        str.push_str(s);
        true
    });
    assert_eq!(str, "HHelloello");
    str = String::new();
    let mut i = 0;
    text.iter(|s| {
        if i == 1 {
            return false;
        }
        str.push_str(s);
        i += 1;
        true
    });
    assert_eq!(str, "H");
}

#[test]
fn test_text_iter_detached() {
    let mut str = String::new();
    let text = TextHandler::new_detached();
    text.insert(0, "Hello").unwrap();
    text.insert(1, "Hello").unwrap();
    text.iter(|s| {
        str.push_str(s);
        true
    });
    assert_eq!(str, "HHelloello");
}

#[test]
fn test_text_update() {
    let doc = LoroDoc::new_auto_commit();
    let text = doc.get_text("text");
    text.insert(0, "Hello 😊Bro").unwrap();
    text.update("Hello World Bro😊", Default::default())
        .unwrap();
    assert_eq!(text.to_string(), "Hello World Bro😊");
}

#[test]
fn test_map_contains_key() {
    let doc = LoroDoc::new_auto_commit();
    let map = doc.get_map("m");
    assert!(!map.contains_key("bro"));
    map.insert("bro", 114514).unwrap();
    assert!(map.contains_key("bro"));
    map.delete("bro").unwrap();
    assert!(!map.contains_key("bro"));
}

#[test]
fn set_max_peer_id() {
    let doc = LoroDoc::new_auto_commit();
    assert_eq!(
        doc.set_peer_id(PeerID::MAX),
        Result::Err(LoroError::InvalidPeerID)
    );
}

#[test]
fn import_status() -> LoroResult<()> {
    let doc = LoroDoc::new_auto_commit();
    doc.set_peer_id(0)?;
    let t = doc.get_text("text");
    t.insert(0, "a")?;

    let doc2 = LoroDoc::new_auto_commit();
    doc2.set_peer_id(1)?;
    let t2 = doc2.get_text("text");
    t2.insert(0, "b")?;
    doc2.commit_then_renew();
    let update1 = doc2.export_snapshot().unwrap();
    let vv1 = doc2.oplog_vv();
    t2.insert(1, "c")?;
    let update2 = doc2.export(ExportMode::updates(&vv1)).unwrap();

    let status1 = doc.import(&update2)?;
    let status2 = doc.import(&update1)?;
    assert_eq!(
        status1,
        ImportStatus {
            success: Default::default(),
            pending: Some(VersionRange::from_map(fx_map!(1=>(1, 2))))
        }
    );
    assert_eq!(
        status2,
        ImportStatus {
            success: VersionRange::from_map(fx_map!(1=>(0, 2))),
            pending: None
        }
    );

    Ok(())
}

#[test]
fn test_on_first_commit_from_peer() {
    let doc = LoroDoc::new_auto_commit();
    doc.set_peer_id(0).unwrap();
    let p = Arc::new(Mutex::new(vec![]));
    let p2 = Arc::clone(&p);
    let sub = doc.subscribe_first_commit_from_peer(Box::new(move |e| {
        p2.try_lock().unwrap().push(e.peer);
        true
    }));
    doc.get_text("text").insert(0, "a").unwrap();
    doc.commit_then_renew();
    doc.get_text("text").insert(0, "b").unwrap();
    doc.commit_then_renew();
    doc.set_peer_id(1).unwrap();
    doc.get_text("text").insert(0, "c").unwrap();
    doc.commit_then_renew();
    sub.unsubscribe();
    assert_eq!(p.try_lock().unwrap().as_slice(), &[0, 1]);
}

#[test]
fn test_on_first_commit_from_peer_when_drop_doc() {
    let doc = LoroDoc::new_auto_commit();
    doc.set_peer_id(0).unwrap();
    let p = Arc::new(Mutex::new(vec![]));
    let p2 = Arc::clone(&p);
    let _sub = doc.subscribe_first_commit_from_peer(Box::new(move |e| {
        p2.try_lock().unwrap().push(e.peer);
        true
    }));
    doc.get_text("text").insert(0, "a").unwrap();
    drop(doc);
    assert_eq!(p.try_lock().unwrap().as_slice(), &[0]);
}

#[test]
fn test_on_first_commit_from_peer_and_set_peer_id() {
    let doc = LoroDoc::new_auto_commit();
    doc.set_peer_id(0).unwrap();
    let f = Arc::new(AtomicBool::new(false));
    let f2 = Arc::clone(&f);
    let sub = doc.subscribe_first_commit_from_peer(Box::new(move |_e| {
        f2.store(true, std::sync::atomic::Ordering::Relaxed);
        true
    }));
    doc.get_text("text").insert(0, "a").unwrap();
    doc.set_peer_id(1).unwrap();
    sub.unsubscribe();
    assert!(f.load(std::sync::atomic::Ordering::Relaxed));
}

#[test]
fn test_on_first_commit_from_peer_with_lock() {
    let doc = LoroDoc::new_auto_commit();
    doc.set_peer_id(0).unwrap();
    let doc_clone = doc.clone();
    let sub = doc.subscribe_first_commit_from_peer(Box::new(move |_e| {
        doc_clone.get_text("text").insert(0, "b").unwrap();
        true
    }));
    doc.get_text("text").insert(0, "a").unwrap();
    doc.commit_then_renew();
    sub.unsubscribe();
    assert_eq!(doc.get_text("text").to_string(), "ba");
    assert_eq!(
        doc.export_json_updates(&Default::default(), &doc.oplog_vv(), false)
            .changes
            .len(),
        1
    );
}

#[test]
fn test_on_first_peer_commit_attach_user_id() {
    let doc = LoroDoc::new_auto_commit();
    doc.set_peer_id(0).unwrap();
    let doc_clone = doc.clone();
    let sub = doc.subscribe_first_commit_from_peer(Box::new(move |e| {
        doc_clone
            .get_map("::loro::user_id")
            .insert(e.peer.to_string().as_str(), "user_bob")
            .unwrap();
        true
    }));
    doc.get_text("text").insert(0, "a").unwrap();
    doc.commit_then_renew();
    sub.unsubscribe();
    assert_eq!(
        doc.get_map("::loro::user_id").get_value(),
        loro_value!({
            "0": "user_bob"
        })
    );
}

#[test]
fn test_pre_commit_with_lock() {
    let doc = LoroDoc::new_auto_commit();
    let doc_clone = doc.clone();
    let sub = doc.subscribe_pre_commit(Box::new(move |_e| {
        // state lock
        doc_clone.get_deep_value();
        // oplog lock
        doc_clone.oplog_vv();
        true
    }));
    doc.get_text("text").insert(0, "a").unwrap();
    doc.commit_then_renew();
    sub.unsubscribe();
}

#[test]
fn test_pre_commit_with_hash() {
    let doc = LoroDoc::new_auto_commit();
    doc.set_peer_id(0).unwrap();
    let doc_clone = doc.clone();
    let sub = doc.subscribe_pre_commit(Box::new(move |e| {
        let change_json =
            doc_clone.export_json_in_id_span(e.change_meta.id.to_span(e.change_meta.len));
        assert!(change_json.len() == 1);
        let mut deps = vec![];
        for dep in e.change_meta.deps.iter() {
            let dep_msg = doc_clone
                .oplog()
                .lock()
                .unwrap()
                .get_change_at(dep)
                .unwrap()
                .message()
                .cloned()
                .unwrap_or(Arc::from(""));
            deps.push(dep_msg);
        }
        e.modifier.set_timestamp(0).set_message(&format!(
            "{:08x}\n{}",
            // just for test example, should use sha256 or blake3 for hash
            xxhash_rust::xxh32::xxh32(
                serde_json::to_string(&(&change_json, &deps))
                    .unwrap()
                    .as_bytes(),
                0
            ),
            e.change_meta.message()
        ));
        true
    }));
    doc.get_text("text").insert(0, "a").unwrap();
    doc.commit_with(
        CommitOptions::default()
            .commit_msg("add a")
            .immediate_renew(true),
    );
    doc.get_text("text").insert(0, "b").unwrap();
    doc.commit_with(
        CommitOptions::default()
            .commit_msg("add b")
            .immediate_renew(true),
    );
    sub.unsubscribe();
    let changes = doc
        .export_json_updates(&Default::default(), &doc.oplog_vv(), false)
        .changes;
    assert_eq!(changes.len(), 2);
    for c in changes {
        let mut msg = c.msg.as_ref().unwrap().lines();
        assert_eq!(msg.next().unwrap().len(), 8);
        assert!(msg.next().is_some());
    }
}

#[test]
fn test_change_to_json_schema_include_uncommit() {
    let doc = LoroDoc::new_auto_commit();
    doc.set_peer_id(0).unwrap();
    doc.get_text("text").insert(0, "a").unwrap();
    doc.commit_then_renew();
    let doc_clone = doc.clone();
    let _sub = doc.subscribe_pre_commit(Box::new(move |e| {
        let changes = doc_clone.export_json_in_id_span(IdSpan::new(
            0,
            0,
            e.change_meta.id.counter + e.change_meta.len as i32,
        ));
        assert_eq!(changes.len(), 2);
        true
    }));
    doc.get_text("text").insert(0, "b").unwrap();
    let changes = doc.export_json_in_id_span(IdSpan::new(0, 0, 2));
    assert_eq!(changes.len(), 1);
    doc.commit_then_renew();
    // change merged
    assert_eq!(changes.len(), 1);
}

#[test]
fn test_ephemeral_store() {
    let store = EphemeralStore::new(1000);
    let store_clone = store.clone();
    let _sub = store.subscribe(Box::new(move |_| {
        store_clone.get_all_states();
        true
    }));
    store.set("a", 1);
    store.set("b", 2);
    store.set("c", 3);
}

#[test]
fn test_origin() {
    let doc = LoroDoc::new_auto_commit();
    doc.set_peer_id(0).unwrap();
    let remote = LoroDoc::new_auto_commit();
    remote.set_peer_id(1).unwrap();
    doc.get_map("map").insert("a", 1).unwrap();
    doc.commit_then_renew();
    let snapshot = doc.export_snapshot().unwrap();
    let expected_origin_string = "expectedOriginString";

    let sub = remote.subscribe_root(Arc::new(move |e| {
        assert_eq!(e.event_meta.origin, expected_origin_string.into());
    }));
    remote
        .import_with(&snapshot, expected_origin_string.into())
        .unwrap();
    sub.unsubscribe();
}

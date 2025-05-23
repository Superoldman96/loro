import { describe, expect, it } from "vitest";
import crypto from "crypto";
import {
  Delta,
  getType,
  ListDiff,
  LoroDoc,
  LoroEventBatch,
  LoroList,
  LoroMap,
  LoroText,
  MapDiff,
  PeerID,
  TextDiff,
  idStrToId
} from "../bundler/index";

describe("event", () => {
  it("target", async () => {
    const loro = new LoroDoc();
    let lastEvent: undefined | LoroEventBatch;
    loro.subscribe((event) => {
      expect(event.by).toBe("local");
      lastEvent = event;
    });
    const text = loro.getText("text");
    const id = text.id;
    text.insert(0, "123");
    loro.commit();
    await oneMs();
    expect(lastEvent?.events[0].target).toEqual(id);
  });

  it("path", async () => {
    const loro = new LoroDoc();
    let lastEvent: undefined | LoroEventBatch;
    loro.subscribe((event) => {
      lastEvent = event;
    });
    const map = loro.getMap("map");
    const subMap = map.setContainer("sub", new LoroMap());
    subMap.set("0", "1");
    loro.commit();
    await oneMs();
    expect(lastEvent?.events[1].path).toStrictEqual(["map", "sub"]);
    const list = subMap.setContainer("list", new LoroList());
    list.insert(0, "2");
    const text = list.insertContainer(1, new LoroText());
    loro.commit();
    await oneMs();
    text.insert(0, "3");
    loro.commit();
    await oneMs();
    expect(lastEvent?.events[0].path).toStrictEqual(["map", "sub", "list", 1]);
  });

  it("text diff", async () => {
    const loro = new LoroDoc();
    let lastEvent: undefined | LoroEventBatch;
    loro.subscribe((event) => {
      lastEvent = event;
    });
    const text = loro.getText("t");
    text.insert(0, "3");
    loro.commit();
    await oneMs();
    expect(lastEvent?.events[0].diff).toStrictEqual({
      type: "text",
      diff: [{ insert: "3" }],
    } as TextDiff);
    text.insert(1, "12");
    loro.commit();
    await oneMs();
    expect(lastEvent?.events[0].diff).toStrictEqual({
      type: "text",
      diff: [{ retain: 1 }, { insert: "12" }],
    } as TextDiff);
  });

  it("list diff", async () => {
    const loro = new LoroDoc();
    let lastEvent: undefined | LoroEventBatch;
    loro.subscribe((event) => {
      lastEvent = event;
    });
    const text = loro.getList("l");
    text.insert(0, "3");
    loro.commit();
    await oneMs();
    expect(lastEvent?.events[0].diff).toStrictEqual({
      type: "list",
      diff: [{ insert: ["3"] }],
    } as ListDiff);
    text.insert(1, "12");
    loro.commit();
    await oneMs();
    expect(lastEvent?.events[0].diff).toStrictEqual({
      type: "list",
      diff: [{ retain: 1 }, { insert: ["12"] }],
    } as ListDiff);
  });

  it("map diff", async () => {
    const loro = new LoroDoc();
    let lastEvent: undefined | LoroEventBatch;
    loro.subscribe((event) => {
      lastEvent = event;
    });
    const map = loro.getMap("m");
    map.set("0", "3");
    map.set("1", "2");
    loro.commit();
    await oneMs();
    expect(lastEvent?.events[0].diff).toStrictEqual({
      type: "map",
      updated: {
        "0": "3",
        "1": "2",
      },
    } as MapDiff);
    map.set("0", "0");
    map.set("1", "1");
    loro.commit();
    await oneMs();
    expect(lastEvent?.events[0].diff).toStrictEqual({
      type: "map",
      updated: {
        "0": "0",
        "1": "1",
      },
    } as MapDiff);
  });

  it("tree", async () => {
    const loro = new LoroDoc();
    let lastEvent: undefined | LoroEventBatch;
    loro.subscribe((event) => {
      lastEvent = event;
    });
    const tree = loro.getTree("tree");
    const id = tree.id;
    tree.createNode();
    loro.commit();
    await oneMs();
    expect(lastEvent?.events[0].target).toEqual(id);
  });

  describe("subscribe container events", () => {
    it("text", async () => {
      const loro = new LoroDoc();
      const text = loro.getText("text");
      let ran = 0;
      const sub = text.subscribe((event) => {
        if (!ran) {
          expect((event.events[0].diff as any).diff).toStrictEqual([
            { insert: "123" },
          ] as Delta<string>[]);
        }
        ran += 1;
        for (const containerDiff of event.events) {
          expect(containerDiff.target).toBe(text.id);
        }
      });

      text.insert(0, "123");
      loro.commit();
      await oneMs();
      text.insert(1, "456");
      loro.commit();
      await oneMs();
      expect(ran).toBeTruthy();
      // subscribeOnce test
      expect(text.toString()).toEqual("145623");

      // unsubscribe
      const oldRan = ran;
      sub();
      text.insert(0, "789");
      loro.commit();
      await oneMs();
      expect(ran).toBe(oldRan);
    });

    it("map subscribe deep", async () => {
      const loro = new LoroDoc();
      const map = loro.getMap("map");
      let times = 0;
      const sub = map.subscribe((event) => {
        times += 1;
      });

      const subMap = map.setContainer("sub", new LoroMap());
      loro.commit();
      await oneMs();
      expect(times).toBe(1);
      const text = subMap.setContainer("k", new LoroText());
      loro.commit();
      await oneMs();
      expect(times).toBe(2);
      text.insert(0, "123");
      loro.commit();
      await oneMs();
      expect(times).toBe(3);

      // unsubscribe
      sub();
      text.insert(0, "123");
      loro.commit();
      await oneMs();
      expect(times).toBe(3);
    });

    it("list subscribe deep", async () => {
      const loro = new LoroDoc();
      const list = loro.getList("list");
      let times = 0;
      const sub = list.subscribe((event) => {
        times += 1;
      });

      const text = list.insertContainer(0, new LoroText());
      loro.commit();
      await oneMs();
      expect(times).toBe(1);
      text.insert(0, "123");
      await oneMs();
      loro.commit();
      await oneMs();
      expect(times).toBe(2);

      // unsubscribe
      sub();
      text.insert(0, "123");
      loro.commit();
      await oneMs();
      expect(times).toBe(2);
    });
  });

  describe("text event length should be utf16", () => {
    it("test", async () => {
      const loro = new LoroDoc();
      const text = loro.getText("text");
      let string = "";
      text.subscribe((event) => {
        for (const containerDiff of event.events) {
          const diff = containerDiff.diff;
          expect(diff.type).toBe("text");
          if (diff.type === "text") {
            let newString = "";
            let pos = 0;
            for (const delta of diff.diff) {
              if (delta.retain != null) {
                newString += string.slice(pos, pos + delta.retain);
                pos += delta.retain;
              } else if (delta.insert != null) {
                newString += delta.insert;
              } else {
                pos += delta.delete;
              }
            }

            string = newString + string.slice(pos);
          }
        }
      });
      text.insert(0, "你好");
      loro.commit();
      await oneMs();
      expect(text.toString()).toBe(string);

      text.insert(1, "世界");
      loro.commit();
      await oneMs();
      expect(text.toString()).toBe(string);

      text.insert(2, "👍");
      loro.commit();
      await oneMs();
      expect(text.toString()).toBe(string);

      text.insert(2, "♪(^∇^*)");
      loro.commit();
      await oneMs();
      expect(text.toString()).toBe(string);
    });
  });

  describe("handler in event", () => {
    it("test", async () => {
      const loro = new LoroDoc();
      const list = loro.getList("list");
      let first = true;
      loro.subscribe((e) => {
        if (first) {
          const diff = (e.events[0].diff as ListDiff).diff;
          const text = diff[0].insert![0] as LoroText;
          text.insert(0, "abc");
          first = false;
        }
      });
      list.insertContainer(0, new LoroText());
      loro.commit();
      await oneMs();
      expect(loro.toJSON().list[0]).toBe("abc");
    });
  });

  it("diff can contain containers", async () => {
    const doc = new LoroDoc();
    const list = doc.getList("list");
    let ran = false;
    doc.subscribe((event) => {
      if (event.events[0].diff.type === "list") {
        for (const item of event.events[0].diff.diff) {
          const t = item.insert![0] as LoroText;
          expect(t.toString()).toBe("Hello");
          expect(item.insert?.length).toBe(2);
          expect(getType(item.insert![0])).toBe("Text");
          expect(getType(item.insert![1])).toBe("Map");
        }
        ran = true;
      }
    });

    list.insertContainer(0, new LoroMap());
    const t = list.insertContainer(0, new LoroText());
    t.insert(0, "He");
    t.insert(2, "llo");
    doc.commit();
    await new Promise((resolve) => setTimeout(resolve, 1));
    expect(ran).toBeTruthy();
  });

  it("remote event", async () => {
    const doc = new LoroDoc();
    const list = doc.getList("list");
    list.insert(0, 123);
    {
      const doc2 = new LoroDoc();
      let triggered = false;
      doc2.subscribe((event) => {
        expect(event.by).toBe("import");
        triggered = true;
      });
      doc2.import(doc.exportFrom());
      await oneMs();
      expect(triggered).toBeTruthy();
    }
    {
      const doc2 = new LoroDoc();
      let triggered = false;
      doc2.subscribe((event) => {
        expect(event.by).toBe("import");
        triggered = true;
      });
      doc2.import(doc.exportSnapshot());
      await oneMs();
      expect(triggered).toBeTruthy();
    }
  });

  it("checkout event", async () => {
    const doc = new LoroDoc();
    const list = doc.getList("list");
    list.insert(0, 123);
    doc.commit();
    let triggered = false;
    doc.subscribe((e) => {
      expect(e.by).toBe("checkout");
      triggered = true;
    });

    doc.checkout([]);
    await oneMs();
    expect(triggered).toBeTruthy();
  });

  describe("local updates events", () => {
    it("basic", () => {
      const loro = new LoroDoc();
      const text = loro.getText("text");
      let updateReceived = false;

      const unsubscribe = loro.subscribeLocalUpdates((update) => {
        updateReceived = true;
        expect(update).toBeInstanceOf(Uint8Array);
        expect(update.length).toBeGreaterThan(0);
      });

      text.insert(0, "Hello");
      loro.commit();

      expect(updateReceived).toBe(true);

      // Test unsubscribe
      updateReceived = false;
      unsubscribe();

      text.insert(5, " World");
      loro.commit();

      expect(updateReceived).toBe(false);
    });

    it("multiple subscribers", () => {
      const loro = new LoroDoc();
      const text = loro.getText("text");
      let count1 = 0;
      let count2 = 0;

      const unsubscribe1 = loro.subscribeLocalUpdates(() => {
        count1++;
      });

      const unsubscribe2 = loro.subscribeLocalUpdates(() => {
        count2++;
      });

      text.insert(0, "Hello");
      loro.commit();

      expect(count1).toBe(1);
      expect(count2).toBe(1);

      unsubscribe1();

      text.insert(5, " World");
      loro.commit();

      expect(count1).toBe(1);
      expect(count2).toBe(2);

      unsubscribe2();
    });

    it("updates for different containers", () => {
      const loro = new LoroDoc();
      const text = loro.getText("text");
      const list = loro.getList("list");
      const map = loro.getMap("map");
      let updates = 0;

      loro.subscribeLocalUpdates(() => {
        updates++;
      });

      text.insert(0, "Hello");
      list.push("World");
      map.set("key", "value");
      loro.commit();

      expect(updates).toBe(1); // All changes are bundled in one update

      text.insert(5, "!");
      loro.commit();

      expect(updates).toBe(2);
    });

    it("can be used to sync", () => {
      const loro1 = new LoroDoc();
      const loro2 = new LoroDoc();
      const text1 = loro1.getText("text");
      const text2 = loro2.getText("text");

      loro1.subscribeLocalUpdates((updates) => {
        loro2.import(updates);
      });

      loro2.subscribeLocalUpdates((updates) => {
        loro1.import(updates);
      });

      text1.insert(0, "Hello");
      loro1.commit();

      expect(text2.toString()).toBe("Hello");

      text2.insert(5, " World");
      loro2.commit();

      expect(text1.toString()).toBe("Hello World");

      // Test concurrent edits
      text1.insert(0, "1. ");
      text2.insert(text2.length, "!");
      loro1.commit();
      loro2.commit();

      // Both documents should converge to the same state
      expect(text1.toString()).toBe("1. Hello World!");
      expect(text2.toString()).toBe("1. Hello World!");
    });
  });
});

it("subscription works after timeout", async () => {
  const doc = new LoroDoc();
  let times = 0;
  doc.subscribe(() => {
    times += 1;
  });

  for (let i = 0; i < 3; i++) {
    if ((globalThis as any).gc) {
      (globalThis as any).gc();
    } else {
      throw new Error("No GC");
    }
    const s = i.toString();
    doc.getText("text").insert(0, s);
    doc.commit();
    await oneMs();
    expect(times).toBe(1);
    times = 0;
    await new Promise((resolve) => setTimeout(resolve, 10));
  }
});

it("subscription for local updates works after timeout", async () => {
  const doc = new LoroDoc();
  let times = 0;
  doc.subscribeLocalUpdates(() => {
    times += 1;
  });

  for (let i = 0; i < 3; i++) {
    if ((globalThis as any).gc) {
      (globalThis as any).gc();
    } else {
      throw new Error("No GC");
    }
    doc.getText("text").insert(0, "h");
    doc.commit();
    await oneMs();
    expect(times).toBe(1);
    times = 0;
    await new Promise((resolve) => setTimeout(resolve, 10));
  }
});

it("subscribe first commit from peer", () => {
  const doc = new LoroDoc();
  doc.setPeerId(0);
  let p: PeerID[] = [];
  doc.subscribeFirstCommitFromPeer((e) => {
    p.push(e.peer);
    doc.getMap("map").set(e.peer, "user-" + e.peer);
  });
  doc.getList("list").insert(0, 100);
  doc.commit();
  doc.getList("list").insert(0, 200);
  doc.commit();
  doc.setPeerId(1);
  doc.getList("list").insert(0, 300);
  doc.commit();
  expect(p).toEqual(["0", "1"]);
  expect(doc.getMap("map").get("0")).toBe("user-0");
});

it("subscribe pre commit", () => {
  const doc = new LoroDoc();
  doc.setPeerId(0);
  doc.subscribePreCommit((e) => {
    e.modifier.setMessage("test").setTimestamp(Date.now());
  });
  doc.getList("list").insert(0, 100);
  doc.commit();
  expect(doc.getChangeAt({ peer: "0", counter: 0 }).message).toBe("test");
});

function oneMs(): Promise<void> {
  return new Promise((r) => setTimeout(r));
}

it("use precommit for storing hash", () => {
  const doc = new LoroDoc();
  doc.setPeerId(0);
  doc.subscribePreCommit((e) => {
    const changes = doc.exportJsonInIdSpan(e.changeMeta)
    expect(changes).toHaveLength(1);
    const hash = crypto.createHash('sha256');
    const change = {
      ...changes[0],
      deps: changes[0].deps.map(d => {
        const depChange = doc.getChangeAt(idStrToId(d))
        return depChange.message;
      })
    }
    console.log(change); // The output is shown below
    hash.update(JSON.stringify(change));
    const sha256Hash = hash.digest('hex');
    e.modifier.setMessage(sha256Hash);
  });

  doc.getList("list").insert(0, 100);
  doc.commit();
  // Change 0
  // {
  //   id: '0@0',
  //   timestamp: 0,
  //   deps: [],
  //   lamport: 0,
  //   msg: undefined,
  //   ops: [
  //     {
  //       container: 'cid:root-list:List',
  //       content: { type: 'insert', pos: 0, value: [100] },
  //       counter: 0
  //     }
  //   ]
  // }


  doc.getList("list").insert(0, 200);
  doc.commit();
  // Change 1
  // {
  //   id: '1@0',
  //   timestamp: 0,
  //   deps: [
  //     '2af99cf93869173984bcf6b1ce5412610b0413d027a5511a8f720a02a4432853'
  //   ],
  //   lamport: 1,
  //   msg: undefined,
  //   ops: [
  //     {
  //       container: 'cid:root-list:List',
  //       content: { type: 'insert', pos: 0, value: [200] },
  //       counter: 1
  //     }
  //   ]
  // }

  expect(doc.getChangeAt({ peer: "0", counter: 0 }).message).toBe("2af99cf93869173984bcf6b1ce5412610b0413d027a5511a8f720a02a4432853");
  expect(doc.getChangeAt({ peer: "0", counter: 1 }).message).toBe("aedbb442c554ecf59090e0e8339df1d8febf647f25cc37c67be0c6e27071d37f");
})

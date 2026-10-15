// 仮の画面。状態を取得して一覧にし、保存と終了のボタンだけを持つ。
// 文字列は textContent だけで入れる(HTML として解釈させない)。
(function () {
  "use strict";
  var manual = {};

  function setStatus(text) {
    document.getElementById("status").textContent = text;
  }

  function fill(listId, lines, cls) {
    var ul = document.getElementById(listId);
    while (ul.firstChild) {
      ul.removeChild(ul.firstChild);
    }
    lines.forEach(function (line) {
      var li = document.createElement("li");
      li.textContent = line;
      if (cls) {
        li.className = cls;
      }
      ul.appendChild(li);
    });
  }

  function post(path, body) {
    return fetch(path, {
      method: "POST",
      credentials: "same-origin",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(body),
    });
  }

  function show(state) {
    manual = {};
    state.items.forEach(function (it) {
      if (it.manual) {
        manual[it.id] = it.manual;
      }
    });
    var scenes = [];
    state.scenes.forEach(function (sc) {
      scenes.push(sc.id + (sc.name ? " " + sc.name : "") + "(カット " + sc.cuts.length + ")");
      sc.cuts.forEach(function (c) {
        scenes.push("  " + c.id + " コマ " + c.frames[0] + "-" + c.frames[1] + (c.asset ? " " + c.asset.kind : " 素材なし"));
      });
    });
    fill("scenes", scenes);
    fill(
      "items",
      state.items.map(function (it) {
        return it.id + " " + it.kind + " コマ " + it.frames[0] + "-" + it.frames[1];
      })
    );
    fill("warnings", state.warnings, "warn");
    setStatus("シーン " + state.scenes.length + " 件、項目 " + state.items.length + " 件、警告 " + state.warnings.length + " 件");
  }

  function load() {
    fetch("/api/state", { credentials: "same-origin" })
      .then(function (r) {
        if (!r.ok) {
          throw new Error("状態を取得できません(" + r.status + ")");
        }
        return r.json();
      })
      .then(show)
      .catch(function (e) {
        setStatus(String(e.message || e));
      });
  }

  document.getElementById("save").addEventListener("click", function () {
    post("/api/save", { manual: manual })
      .then(function (r) {
        return r.json().then(function (j) {
          setStatus(r.ok ? "保存しました" : "保存できません: " + (j.error || (j.errors || []).join(" / ")));
        });
      })
      .catch(function (e) {
        setStatus(String(e.message || e));
      });
  });

  document.getElementById("quit").addEventListener("click", function () {
    post("/api/quit", {})
      .then(function () {
        setStatus("終了しました。このタブは閉じてかまいません");
      })
      .catch(function (e) {
        setStatus(String(e.message || e));
      });
  });

  load();
})();

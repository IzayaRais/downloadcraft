// Welcome / problem page: what works, what does not, and how to fix it.
(() => {
  "use strict";
  const api = (typeof browser !== "undefined" && browser.runtime) ? browser : chrome;
  const $ = (id) => document.getElementById(id);
  const set = (key, state, title, text) => {
    $(`d-${key}`).className = `dot ${state}`;
    $(`t-${key}`).textContent = title;
    $(`s-${key}`).textContent = text ? ` ${text}` : "";
  };

  async function check() {
    set("app", "", "Checking the app…", "");
    let st = null;
    try { st = await api.runtime.sendMessage({ type: "getState" }); } catch (e) { /* worker starting */ }
    if (st && st.hostOk) {
      set("app", "ok", "The app is connected", "Downloads and videos go to Download Craft.");
      $("fix-app").classList.add("hidden");
    } else {
      set("app", "bad", "The app is not reachable", (st && st.hostError) || "");
      $("fix-app").classList.remove("hidden");
    }
    if (st && st.live) set("live", "ok", "Live connection", "The app sends its capture settings and tells the browser when downloads finish.");
    else if (st && st.hostOk) set("live", "warn", "Basic connection", "Update the app for the live connection (settings sync, notifications, fallbacks).");
    else set("live", "", "Live connection", "Starts when the app is connected.");
    let incognito = false;
    try { incognito = await api.extension.isAllowedIncognitoAccess(); } catch (e) {}
    if (incognito) set("inc", "ok", "Private windows", "The extension also works in private windows.");
    else set("inc", "warn", "Private windows", "Off. To use it there too, open the extension's details and allow it in private/incognito windows.");
  }

  const reason = new URLSearchParams(location.search).get("reason");
  if (reason === "no-app") $("sub").textContent = "The extension cannot reach the Download Craft app.";
  $("recheck").addEventListener("click", check);
  check();
})();

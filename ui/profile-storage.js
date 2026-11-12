// Injected by the native host before page scripts, including Quick capture.
// Older macOS versions cannot select a WKWebView data store. Keep preferences
// isolated there too; normal release runs retain their existing storage keys.
if (window.terrariumProfileId) {
  window.terrariumStorage = {
    getItem(key) { return localStorage.getItem(`${window.terrariumProfileId}:${key}`); },
    setItem(key, value) { localStorage.setItem(`${window.terrariumProfileId}:${key}`, value); },
    removeItem(key) { localStorage.removeItem(`${window.terrariumProfileId}:${key}`); },
  };
}

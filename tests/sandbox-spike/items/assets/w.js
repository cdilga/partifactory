// Worker body for item 5. Started either from a blob: URL (the prelude's Worker wrapper) or
// directly from its relative URL (expected to fail from an opaque origin).
self.onmessage = async function (e) {
  var out = { pong: e.data.ping, origin: self.origin, loc: String(self.location).slice(0, 48) };
  try {
    var r = await fetch(e.data.abs);
    out.fetch = r.status;
  } catch (err) {
    out.fetch = 'ERR ' + err.name;
  }
  try {
    importScripts(e.data.imp);
    out.imp = self.__imp === 1 ? 'ok' : 'not run';
  } catch (err) {
    out.imp = 'ERR ' + err.name;
  }
  postMessage(out);
};

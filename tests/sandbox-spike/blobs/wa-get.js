// Item 13b: a WebAuthn assertion from the wrapper origin https://a-part.<apex> for a credential
// registered on https://parts.<apex>, both with rp.id = <apex>. Verifies clientData.origin and
// the rpIdHash in authenticatorData.
(function () {
  var q = new URLSearchParams(location.search);
  var wa = q.get('wa') || 'none';
  var first = location.hostname.split('.')[0];
  var apex = location.hostname.slice(first.length + 1);
  var WS = 'https://parts.' + location.host.slice(first.length + 1);
  function fromB64u(s) { s = s.replace(/-/g, '+').replace(/_/g, '/'); while (s.length % 4) s += '='; return Uint8Array.from(atob(s), function (c) { return c.charCodeAt(0); }); }
  function hex(b) { return Array.from(new Uint8Array(b)).map(function (x) { return x.toString(16).padStart(2, '0'); }).join(''); }
  function timeout(p, ms) { return Promise.race([p, new Promise(function (_, rej) { setTimeout(function () { rej(new Error('timeout ' + ms + 'ms')); }, ms); })]); }
  async function run() {
    var R = [];
    try {
      var creds = await (await fetch(WS + '/_spike/wa/creds')).json();
      if (!window.PublicKeyCredential) throw new Error('PublicKeyCredential absent');
      var a = await timeout(navigator.credentials.get({ publicKey: {
        rpId: apex,
        challenge: crypto.getRandomValues(new Uint8Array(32)),
        allowCredentials: creds.map(function (id) { return { type: 'public-key', id: fromB64u(id) }; }),
        userVerification: 'preferred',
        timeout: 15000,
      } }), 20000);
      var cd = JSON.parse(new TextDecoder().decode(a.response.clientDataJSON));
      var rpHash = hex(await crypto.subtle.digest('SHA-256', new TextEncoder().encode(apex)));
      var got = hex(a.response.authenticatorData.slice(0, 32));
      var ok = cd.origin === location.origin && cd.type === 'webauthn.get' && got === rpHash;
      R.push({ item: '13', check: 'assert_from_wrapper_origin_rpid_apex', outcome: ok ? 'pass' : 'fail',
        detail: JSON.stringify({ origin: cd.origin, rpIdHashMatches: got === rpHash, registered: creds.length }) });
    } catch (e) {
      R.push({ item: '13', check: 'assert_from_wrapper_origin_rpid_apex', outcome: wa === 'virt' ? 'fail' : (wa === 'sim' ? 'not_assertable' : 'not_assertable'),
        detail: e.name + ': ' + e.message });
    }
    document.getElementById('out').textContent = JSON.stringify(R, null, 1);
    try {
      await fetch(WS + '/_spike/result', { method: 'POST', body: JSON.stringify({ type: 'spike:results', lane: q.get('lane'), run: q.get('run'), ua: navigator.userAgent, results: R }) });
    } catch (e) {}
    await new Promise(function (r) { setTimeout(r, 1500); });
    var back = q.get('back');
    if (back) location.replace(back);
  }
  document.addEventListener('DOMContentLoaded', run);
})();

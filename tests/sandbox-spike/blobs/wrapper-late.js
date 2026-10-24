// "Any other script" in the wrapper: runs after wrapper.js. The #pf= fragment must already be
// gone from the URL by the time this runs.
window.__pfLateSaw = { hash: location.hash, href: location.href.replace(/pf=[^&]*/, 'pf=<cap>') };

import type { WebContents } from 'electron'
import type { ActionResult, AgentViewContext, ScrollDirection } from '@shared/types'
import { buildExtractScript } from '../agent-view/extract'

const MAX_ELEMENTS = 150
const MAX_TEXT_CHARS = 6000

/** Strip a leading "@" so "@e3" -> "e3" (the live data-biscuit-ref value). */
function bareRef(ref: string): string {
  return ref.replace(/^@/, '').trim()
}

// Page-world helper that resolves a data-biscuit-ref across the main document,
// OPEN shadow roots, and same-origin frames — matching where the extractor tags
// elements. Injected as a string into the click/type scripts below.
const REF_FINDER = `function biscuitFind(ref){
    function search(root){
      var hit = null;
      try { hit = root.querySelector('[data-biscuit-ref="' + ref + '"]'); } catch(e){}
      if (hit) return hit;
      var all;
      try { all = root.querySelectorAll('*'); } catch(e){ return null; }
      for (var i=0;i<all.length;i++){
        var el = all[i];
        if (el.shadowRoot){ var r = search(el.shadowRoot); if (r) return r; }
        if (el.tagName === 'IFRAME' || el.tagName === 'FRAME'){
          try { var d = el.contentDocument; if (d){ var r2 = search(d); if (r2) return r2; } } catch(e){}
        }
      }
      return null;
    }
    return search(document);
  }`

/** Run the Agent View extractor in the page and return its raw snapshot. */
export async function runExtract(
  wc: WebContents,
  generation: number
): Promise<{
  url: string
  title: string
  headings: { level: number; text: string }[]
  elements: unknown[]
  text: string
  truncated: boolean
  context?: AgentViewContext
}> {
  const script = buildExtractScript({ generation, maxElements: MAX_ELEMENTS, maxTextChars: MAX_TEXT_CHARS })
  // `userGesture=true` lets the script behave as if user-initiated where needed.
  return wc.executeJavaScript(script, true)
}

export async function clickRef(wc: WebContents, ref: string, generation: number): Promise<ActionResult> {
  const bare = JSON.stringify(bareRef(ref))
  const gen = JSON.stringify(String(generation))
  const script = `(function(ref, gen){
    ${REF_FINDER}
    // Fast reject after a top-frame navigation (fresh doc has no/old gen stamp).
    if (document.documentElement.getAttribute('data-biscuit-gen') !== gen) return {ok:false, detail:'refs expired (page changed) — call refreshAgentView'};
    var el = biscuitFind(ref);
    if (!el) return {ok:false, detail:'ref @'+ref+' not found — call refreshAgentView'};
    // Validate against the element's OWN document so a stale ref inside a frame
    // (whose document carries an older gen) is rejected too.
    var od = el.ownerDocument;
    if (!od || !od.documentElement || od.documentElement.getAttribute('data-biscuit-gen') !== gen) return {ok:false, detail:'refs expired (page changed) — call refreshAgentView'};
    try { el.scrollIntoView({block:'center', inline:'center'}); } catch(e){}
    // Covered-element check: if something else (a modal/cookie wall/overlay)
    // sits over the target's center, clicking would hit the overlay instead.
    // ownerDocument === document is true for top-document light DOM AND shadow
    // elements (shadow doesn't change ownerDocument) but false for iframe
    // elements, whose coordinates would be frame-relative — so skip those.
    if (el.ownerDocument === document) {
      // Containment across shadow boundaries: Node.contains() does NOT pierce
      // host->shadow, so a same-widget hit would otherwise read as "covered".
      var composedContains = function(a, b){
        var n = b;
        while (n){
          if (n === a) return true;
          if (n.nodeType === 11 && n.host) { n = n.host; continue; } // shadow root -> host
          n = n.parentNode;
        }
        return false;
      };
      try {
        var rect = el.getBoundingClientRect();
        if (rect.width > 0 && rect.height > 0) {
          var cx = rect.left + rect.width / 2, cy = rect.top + rect.height / 2;
          if (cx >= 0 && cy >= 0 && cx <= (window.innerWidth||0) && cy <= (window.innerHeight||0)) {
            // Descend through open shadow roots to the deepest element at the point.
            var top = document.elementFromPoint(cx, cy);
            while (top && top.shadowRoot) {
              var inner = top.shadowRoot.elementFromPoint(cx, cy);
              if (!inner || inner === top) break;
              top = inner;
            }
            if (top && top !== el && !composedContains(el, top) && !composedContains(top, el)) {
              var d = top.tagName ? top.tagName.toLowerCase() : 'element';
              if (top.className && typeof top.className === 'string') {
                var cls = top.className.trim().split(/\\s+/).slice(0,2).join('.');
                if (cls) d += '.' + cls;
              }
              return {ok:false, detail:'@'+ref+' is covered by <'+d+'> — dismiss the overlay/cookie wall, then refreshAgentView'};
            }
          }
        }
      } catch(e){ /* best-effort; fall through to click */ }
    }
    // Build a human label from the element text/labels only — never the field
    // contents, which could leak a password / card number into the log, the UI,
    // and the model's recent-actions buffer.
    var label = ((el.innerText||el.getAttribute('aria-label')||el.getAttribute('placeholder')||el.getAttribute('name')||'')+'').replace(/\\s+/g,' ').trim().slice(0,80);
    try { el.click(); } catch(e){ return {ok:false, detail:'click failed: '+e.message}; }
    return {ok:true, detail:'clicked @'+ref+(label?' ('+label+')':'')};
  })(${bare}, ${gen})`
  return wc.executeJavaScript(script, true)
}

export async function typeRef(
  wc: WebContents,
  ref: string,
  text: string,
  generation: number
): Promise<ActionResult> {
  const bare = JSON.stringify(bareRef(ref))
  const gen = JSON.stringify(String(generation))
  const value = JSON.stringify(text)
  const script = `(function(ref, value, gen){
    ${REF_FINDER}
    if (document.documentElement.getAttribute('data-biscuit-gen') !== gen) return {ok:false, detail:'refs expired (page changed) — call refreshAgentView'};
    var el = biscuitFind(ref);
    if (!el) return {ok:false, detail:'ref @'+ref+' not found — call refreshAgentView'};
    var od = el.ownerDocument;
    if (!od || !od.documentElement || od.documentElement.getAttribute('data-biscuit-gen') !== gen) return {ok:false, detail:'refs expired (page changed) — call refreshAgentView'};
    try { el.focus(); } catch(e){}
    try {
      if (el.tagName === 'SELECT') {
        // Selecting an option, not typing free text — match by value or label.
        var matched = -1;
        for (var i = 0; i < el.options.length; i++) {
          var op = el.options[i];
          if (op.value === value || ((op.text||'')+'').trim() === value) { matched = i; break; }
        }
        if (matched === -1) return {ok:false, detail:'no <select> option matches "'+value+'" — choose an existing option'};
        el.selectedIndex = matched;
      } else if (el.tagName === 'INPUT' || el.tagName === 'TEXTAREA') {
        var proto = el.tagName === 'TEXTAREA' ? window.HTMLTextAreaElement.prototype : window.HTMLInputElement.prototype;
        var desc = Object.getOwnPropertyDescriptor(proto, 'value');
        if (desc && desc.set) { desc.set.call(el, value); } else { el.value = value; }
      } else if (el.isContentEditable) {
        el.textContent = value;
      } else {
        return {ok:false, detail:'@'+ref+' is not an editable field'};
      }
      el.dispatchEvent(new Event('input', { bubbles: true }));
      el.dispatchEvent(new Event('change', { bubbles: true }));
    } catch(e) { return {ok:false, detail:'type failed: '+e.message}; }
    return {ok:true, detail: el.tagName === 'SELECT' ? 'selected option in @'+ref : 'typed '+value.length+' chars into @'+ref};
  })(${bare}, ${value}, ${gen})`
  return wc.executeJavaScript(script, true)
}

export async function scroll(
  wc: WebContents,
  direction: ScrollDirection,
  pages: number
): Promise<ActionResult> {
  const dir = JSON.stringify(direction)
  const n = JSON.stringify(Math.max(1, Math.floor(pages || 1)))
  const script = `(function(direction, pages){
    var h = window.innerHeight || 800;
    if (direction === 'top') { window.scrollTo(0,0); return {ok:true, detail:'scrolled to top'}; }
    if (direction === 'bottom') { window.scrollTo(0, document.body ? document.body.scrollHeight : 0); return {ok:true, detail:'scrolled to bottom'}; }
    var delta = (direction === 'up' ? -1 : 1) * pages * h * 0.9;
    window.scrollBy(0, delta);
    return {ok:true, detail:'scrolled '+direction+' '+pages+' page(s); y='+Math.round(window.scrollY)};
  })(${dir}, ${n})`
  return wc.executeJavaScript(script, true)
}

/** Fallback only — capture a PNG data URL of the visible page. */
export async function screenshot(wc: WebContents): Promise<ActionResult> {
  try {
    const image = await wc.capturePage()
    return { ok: true, detail: 'captured screenshot (fallback)', data: image.toDataURL() }
  } catch (err) {
    return { ok: false, detail: `screenshot failed: ${(err as Error).message}` }
  }
}

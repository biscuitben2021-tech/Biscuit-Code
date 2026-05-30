import type { WebContents } from 'electron'
import type { ActionResult, ScrollDirection } from '@shared/types'
import { buildExtractScript } from '../agent-view/extract'

const MAX_ELEMENTS = 150
const MAX_TEXT_CHARS = 6000

/** Strip a leading "@" so "@e3" -> "e3" (the live data-biscuit-ref value). */
function bareRef(ref: string): string {
  return ref.replace(/^@/, '').trim()
}

/** Run the Agent View extractor in the page and return its raw snapshot. */
export async function runExtract(wc: WebContents, generation: number): Promise<{
  url: string
  title: string
  headings: { level: number; text: string }[]
  elements: unknown[]
  text: string
  truncated: boolean
}> {
  const script = buildExtractScript({ generation, maxElements: MAX_ELEMENTS, maxTextChars: MAX_TEXT_CHARS })
  // `userGesture=true` lets the script behave as if user-initiated where needed.
  return wc.executeJavaScript(script, true)
}

export async function clickRef(wc: WebContents, ref: string, generation: number): Promise<ActionResult> {
  const bare = JSON.stringify(bareRef(ref))
  const gen = JSON.stringify(String(generation))
  const script = `(function(ref, gen){
    var root = document.documentElement;
    if (root.getAttribute('data-biscuit-gen') !== gen) return {ok:false, detail:'refs expired (page changed) — call refreshAgentView'};
    var el = document.querySelector('[data-biscuit-ref="' + ref + '"]');
    if (!el) return {ok:false, detail:'ref @'+ref+' not found — call refreshAgentView'};
    try { el.scrollIntoView({block:'center', inline:'center'}); } catch(e){}
    var label = ((el.innerText||el.value||el.getAttribute('aria-label')||'')+'').replace(/\\s+/g,' ').trim().slice(0,80);
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
    var root = document.documentElement;
    if (root.getAttribute('data-biscuit-gen') !== gen) return {ok:false, detail:'refs expired (page changed) — call refreshAgentView'};
    var el = document.querySelector('[data-biscuit-ref="' + ref + '"]');
    if (!el) return {ok:false, detail:'ref @'+ref+' not found — call refreshAgentView'};
    try { el.focus(); } catch(e){}
    try {
      if (el.tagName === 'INPUT' || el.tagName === 'TEXTAREA') {
        var proto = el.tagName === 'TEXTAREA' ? window.HTMLTextAreaElement.prototype : window.HTMLInputElement.prototype;
        var desc = Object.getOwnPropertyDescriptor(proto, 'value');
        if (desc && desc.set) { desc.set.call(el, value); } else { el.value = value; }
      } else if (el.isContentEditable) {
        el.textContent = value;
      } else {
        el.value = value;
      }
      el.dispatchEvent(new Event('input', { bubbles: true }));
      el.dispatchEvent(new Event('change', { bubbles: true }));
    } catch(e) { return {ok:false, detail:'type failed: '+e.message}; }
    return {ok:true, detail:'typed '+value.length+' chars into @'+ref};
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

import type { ActionKind, ActionProposal, PageSignature, VerifyResult } from '@shared/types'

// The verification layer. After a mutating action runs, the runtime captures a
// fresh PageSignature and compares it to the one taken just before. This pure
// function turns that before/after pair into a verdict: did the page actually
// change, and did the action appear to fail (new error banner / form validation
// failure)? The runtime feeds the verdict back into its state so the model can
// react instead of confidently continuing on a no-op.

/** Action kinds that are expected to change the page in some observable way. */
const MUTATING: ReadonlySet<ActionKind> = new Set<ActionKind>(['openUrl', 'clickRef', 'typeRef', 'scroll'])

/** Action kinds for which "nothing changed" is suspicious (genuine no-op risk). */
const EXPECTS_CHANGE: ReadonlySet<ActionKind> = new Set<ActionKind>(['openUrl', 'clickRef', 'typeRef'])

export function isMutating(kind: ActionKind): boolean {
  return MUTATING.has(kind)
}

/** Whether the runtime should capture before/after signatures for this action. */
export function shouldVerify(kind: ActionKind): boolean {
  return EXPECTS_CHANGE.has(kind)
}

/**
 * Compare two page fingerprints and judge whether `proposal` took effect.
 * Pure and deterministic — unit-tested in test/verify.test.ts.
 */
export function verifyAction(
  before: PageSignature,
  after: PageSignature,
  proposal: ActionProposal
): VerifyResult {
  const navigated = before.url !== after.url
  const titleChanged = before.title !== after.title
  const contentChanged =
    before.textHash !== after.textHash ||
    Math.abs(after.textLength - before.textLength) > 20 ||
    before.interactiveCount !== after.interactiveCount
  const changed = navigated || titleChanged || contentChanged

  // Error / validation signals that are NEW since before the action.
  const beforeAlerts = new Set(before.alerts)
  const newAlerts = after.alerts.filter((a) => !beforeAlerts.has(a))
  // Typing naturally toggles :invalid as a partial value is entered, so a rising
  // invalid count is only treated as a failure for submit/navigate actions.
  const moreInvalid = after.invalidFields > before.invalidFields && proposal.kind !== 'typeRef'

  const warnings: string[] = []
  let ok = true

  if (newAlerts.length > 0) {
    ok = false
    warnings.push(`page reported: "${newAlerts.slice(0, 3).join('" | "')}"`)
  }
  if (moreInvalid) {
    ok = false
    const delta = after.invalidFields - before.invalidFields
    warnings.push(`form validation failed (${delta} more invalid field${delta === 1 ? '' : 's'})`)
  }
  if (EXPECTS_CHANGE.has(proposal.kind) && !changed && ok) {
    // If the page is still loading/rendering, "no change yet" is expected, not
    // a failure — say so rather than crying no-op.
    if (after.busy) {
      warnings.push('page is still loading/rendering — could not yet confirm the action took effect')
    } else {
      warnings.push(
        'no visible change after this action — it may not have taken effect ' +
          '(the control may be disabled/covered, or you may need to refreshAgentView)'
      )
    }
  }

  return { changed, ok, summary: summarize(proposal.kind, changed, navigated, titleChanged, ok), warnings }
}

function summarize(
  kind: ActionKind,
  changed: boolean,
  navigated: boolean,
  titleChanged: boolean,
  ok: boolean
): string {
  if (!ok) return 'verify: action appears to have FAILED (see warnings)'
  if (navigated) return 'verify: page navigated'
  if (changed) return titleChanged ? 'verify: page content + title changed' : 'verify: page content changed'
  return EXPECTS_CHANGE.has(kind) ? 'verify: no observable change' : 'verify: ok'
}

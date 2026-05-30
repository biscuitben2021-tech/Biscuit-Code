import { useEffect, useRef } from 'react'

/**
 * Placeholder for the native WebContentsView. The actual browsed page is
 * rendered by the main process in a native view layered exactly over this box.
 * We measure this element and report its bounds so main can position the view.
 */
export function BrowserViewport({ hasTab, hidden }: { hasTab: boolean; hidden?: boolean }): JSX.Element {
  const ref = useRef<HTMLDivElement>(null)

  useEffect(() => {
    const el = ref.current
    if (!el) return

    const report = (): void => {
      // The native WebContentsView paints ABOVE the DOM, so it would occlude a
      // modal/overlay. When `hidden`, collapse it to zero size so DOM dialogs
      // are visible.
      if (hidden) {
        void window.biscuit.view.setBounds({ x: 0, y: 0, width: 0, height: 0 })
        return
      }
      const r = el.getBoundingClientRect()
      void window.biscuit.view.setBounds({
        x: Math.round(r.left),
        y: Math.round(r.top),
        width: Math.round(r.width),
        height: Math.round(r.height)
      })
    }

    report()
    const ro = new ResizeObserver(report)
    ro.observe(el)
    window.addEventListener('resize', report)
    // Re-report shortly after mount in case fonts/layout settle.
    const t = window.setTimeout(report, 150)
    return () => {
      ro.disconnect()
      window.removeEventListener('resize', report)
      window.clearTimeout(t)
    }
  }, [hidden])

  return (
    <div className="viewport" ref={ref}>
      {!hasTab && <div className="hint">Open a tab to start browsing</div>}
    </div>
  )
}

import { useEffect, useRef } from 'react'

/**
 * Placeholder for the native WebContentsView. The actual browsed page is
 * rendered by the main process in a native view layered exactly over this box.
 * We measure this element and report its bounds so main can position the view.
 */
export function BrowserViewport({ hasTab }: { hasTab: boolean }): JSX.Element {
  const ref = useRef<HTMLDivElement>(null)

  useEffect(() => {
    const el = ref.current
    if (!el) return

    const report = (): void => {
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
  }, [])

  return (
    <div className="viewport" ref={ref}>
      {!hasTab && <div className="hint">Open a tab to start browsing</div>}
    </div>
  )
}

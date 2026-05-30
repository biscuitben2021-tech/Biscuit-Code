import type { BiscuitApi } from '@shared/api'

declare global {
  interface Window {
    biscuit: BiscuitApi
  }
}

export {}

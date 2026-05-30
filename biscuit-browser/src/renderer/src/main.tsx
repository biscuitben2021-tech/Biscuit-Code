import { StrictMode } from 'react'
import { createRoot } from 'react-dom/client'
import { App } from './App'
import { init } from './state/store'
import './styles.css'

void init()

const root = document.getElementById('root')
if (root) {
  createRoot(root).render(
    <StrictMode>
      <App />
    </StrictMode>
  )
}

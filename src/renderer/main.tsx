import React from 'react'
import ReactDOM from 'react-dom/client'
import App from './App'
import { isMac } from './lib/utils'
import './globals.css'

// macOS keeps its native overlay scrollbars and the traffic-light insets.
if (isMac) document.documentElement.classList.add('is-mac')

ReactDOM.createRoot(document.getElementById('root')!).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>
)

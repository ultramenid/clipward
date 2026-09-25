import React from 'react'
import ReactDOM from 'react-dom/client'
import App from './App'
import { isMac } from './lib/utils'
import './globals.css'

// macOS windows have native vibrancy; the backdrop turns translucent over it.
if (isMac) document.documentElement.classList.add('vibrant')

ReactDOM.createRoot(document.getElementById('root')!).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>
)

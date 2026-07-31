import { defineConfig } from 'vite'
import react from '@vitejs/plugin-react'

// https://vite.dev/config/
export default defineConfig({
  plugins: [react()],
  server: {
    port: 1420,
    strictPort: true,
    watch: {
      // Cargo writes into src-tauri/target; watching it crashes Vite with EBUSY.
      ignored: ['**/src-tauri/**'],
    },
  },
})

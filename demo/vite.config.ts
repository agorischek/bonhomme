import { defineConfig } from 'vite'
import react from '@vitejs/plugin-react'

// https://vite.dev/config/
export default defineConfig({
  plugins: [react()],
  server: {
    proxy: {
      '/api': {
        target: process.env.BONHOMME_API_BASE ?? 'http://127.0.0.1:3030',
      },
    },
  },
})

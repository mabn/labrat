import { defineConfig } from 'vite';
import react from '@vitejs/plugin-react';

// `npm run dev` serves the app on :5173 and forwards labrat's paths to a labrat running on
// :8080 (override with LABRAT_URL).
const target = process.env.LABRAT_URL ?? 'http://127.0.0.1:8080';
const backend = {
  target,
  changeOrigin: false,
};

export default defineConfig({
  plugins: [react()],
  build: {
    assetsDir: 'assets/spa',
    sourcemap: false,
    chunkSizeWarningLimit: 1500,
  },
  server: {
    proxy: {
      '/api': backend,
      '/uploads': backend,
      '/-/labrat/requests.json': backend,
      '/-/labrat/reset': backend,
      // Sign-in and sign-out are form posts handled by labrat; their GET pages are the app's.
      '/users/sign_in': { ...backend, bypass: (req) => (req.method === 'GET' ? '/index.html' : undefined) },
      '/users/sign_out': backend,
    },
  },
});

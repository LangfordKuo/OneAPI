import { fileURLToPath, URL } from 'node:url';
import tailwindcss from '@tailwindcss/vite';
import react from '@vitejs/plugin-react';
import { defineConfig, type ProxyOptions } from 'vite';

const backend = 'http://127.0.0.1:3000';

// The backend rejects mutating requests whose Origin differs from Host, so the
// dev proxy rewrites Origin to the backend it is forwarding to.
const proxy: ProxyOptions = {
  target: backend,
  changeOrigin: true,
  configure(instance) {
    instance.on('proxyReq', (proxyReq) => {
      proxyReq.setHeader('origin', backend);
    });
  },
};

export default defineConfig({
  plugins: [react(), tailwindcss()],
  resolve: {
    alias: {
      '@': fileURLToPath(new URL('./src', import.meta.url)),
    },
  },
  server: {
    port: 5173,
    proxy: {
      '/api': proxy,
      '/v1': proxy,
      '/healthz': proxy,
      '/readyz': proxy,
    },
  },
  build: {
    outDir: 'dist',
    sourcemap: false,
    rollupOptions: {
      output: {
        manualChunks: {
          react: ['react', 'react-dom', 'react-router-dom'],
          query: ['@tanstack/react-query'],
          charts: ['recharts'],
        },
      },
    },
  },
});

import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";

export default defineConfig({
  // relative asset paths, so a reverse proxy can serve the dashboard under any sub-path
  base: "./",
  plugins: [react(), tailwindcss()],
  server: { proxy: { "/api": "http://localhost:8090" } },
});

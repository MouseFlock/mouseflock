import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import App from "./App";
import { tauriBridge } from "./bridge";
import "./index.css";

document.body.dataset.window = tauriBridge.windowLabel();

createRoot(document.getElementById("root")!).render(
  <StrictMode>
    <App />
  </StrictMode>,
);

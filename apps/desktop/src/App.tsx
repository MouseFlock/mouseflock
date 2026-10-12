// The UI is a pipe client of the service, through the Rust side. No engine
// logic here. One frontend serves both windows, picked by window label.
import { MainWindow } from "./MainWindow";
import { UndoPopup } from "./UndoPopup";
import { useBridge } from "./useMouseFlock";

export default function App() {
  return useBridge().windowLabel() === "undo" ? <UndoPopup /> : <MainWindow />;
}

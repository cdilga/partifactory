// A ~50-line Claude-style component: state, effects, a list, TS types.
import { useEffect, useMemo, useState } from "react";
import { createRoot } from "react-dom/client";
// @chart

type Item = { id: number; text: string; done: boolean };

function useStored<T>(key: string, initial: T): [T, (v: T) => void] {
  const [v, setV] = useState<T>(() => {
    try {
      const s = localStorage.getItem(key);
      return s ? (JSON.parse(s) as T) : initial;
    } catch {
      return initial;
    }
  });
  useEffect(() => {
    localStorage.setItem(key, JSON.stringify(v));
  }, [key, v]);
  return [v, setV];
}

export function App({ title = "Packing list" }: { title?: string }) {
  const [items, setItems] = useStored<Item[]>("items", []);
  const [text, setText] = useState("");
  const left = useMemo(() => items.filter((i) => !i.done).length, [items]);
  const add = () => {
    if (!text.trim()) return;
    setItems([...items, { id: Date.now(), text, done: false }]);
    setText("");
  };
  return (
    <main style={{ fontFamily: "system-ui", maxWidth: 480, margin: "2rem auto" }}>
      <h1>{title}</h1>
      <p>{left} left of {items.length}</p>
      <input value={text} onChange={(e) => setText(e.target.value)} onKeyDown={(e) => e.key === "Enter" && add()} />
      <button onClick={add}>Add</button>
      <ul>
        {items.map((i) => (
          <li key={i.id} onClick={() => setItems(items.map((x) => (x.id === i.id ? { ...x, done: !x.done } : x)))}>
            {i.done ? <s>{i.text}</s> : i.text}
          </li>
        ))}
      </ul>
    </main>
  );
}

createRoot(document.getElementById("root")!).render(<App />);

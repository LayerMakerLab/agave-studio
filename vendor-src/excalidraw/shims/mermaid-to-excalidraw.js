// Agave leaves Mermaid out of the board bundle: it is large, and parts of its dependency tree are not under the
// licences Agave ships (MIT, Apache-2.0, BSD, ISC). Excalidraw's "Mermaid to Excalidraw" dialog says it failed.
export const parseMermaidToExcalidraw = async () => {
  throw new Error("Mermaid diagrams are not available in Agave Studio's board.");
};

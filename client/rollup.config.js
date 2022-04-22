import { nodeResolve } from "@rollup/plugin-node-resolve";
import { terser } from "rollup-plugin-terser";

export default {
  input: "src/index.js",
  output: {
    compact: true,
    format: "cjs",
  },
  plugins: [nodeResolve(), terser()],
};

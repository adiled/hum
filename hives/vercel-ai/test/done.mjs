// A script that asks once and is done, under Node as an app runs the
// provider. It closes the provider when told to; otherwise it just ends.

import { generateText } from "ai";
import { createHum } from "../dist/index.js";

const hum = createHum({ label: "script" });
const { text } = await generateText({ model: hum("lab-model"), prompt: "bye" });
console.log(`done: ${text}`);
if (process.argv[2] === "close") await hum.close();

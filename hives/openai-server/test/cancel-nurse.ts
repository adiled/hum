// A nurse for the end-to-end tests that grows nothing and says each cancel
// humd hands it, as every nurse here is handed every cancel.

import { Bee, Chi, Role } from "../../../thrum-clients/ts/index.ts";

const bee = new Bee({ hive: "openai-server", as: "lab-cancels", role: Role.nurse, hello: { models: [], headroom: { freeSlots: 0, totalSlots: 0 } } });
bee.on(Chi.cancel, (t) => console.log(`cancel ${t.sid ?? ""}`));
await bee.start();
console.log("cancels nestled");
await bee.closed().catch(() => {});

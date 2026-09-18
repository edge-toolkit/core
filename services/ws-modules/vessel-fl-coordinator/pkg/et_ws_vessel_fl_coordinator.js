import { createVesselFlUi } from "/modules/et-ws-pyo3-vessel-fl-demo/vessel_fl_ui.js";

const workflow = createVesselFlUi({ role: "coordinator", clientId: null, label: "coordinator" });

export default workflow.init;
export const run = workflow.run;
export const stop = workflow.stop;
export const is_running = workflow.isRunning;

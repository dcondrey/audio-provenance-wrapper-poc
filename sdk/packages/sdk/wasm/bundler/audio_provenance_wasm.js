/* @ts-self-types="./audio_provenance_wasm.d.ts" */
import * as wasm from "./audio_provenance_wasm_bg.wasm";
import { __wbg_set_wasm } from "./audio_provenance_wasm_bg.js";

__wbg_set_wasm(wasm);
wasm.__wbindgen_start();
export {
    capabilities, inspectBytes, locators, start, statuses, verifyBytes, version
} from "./audio_provenance_wasm_bg.js";

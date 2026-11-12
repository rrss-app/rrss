import { SCENE_NAMES, SCENES } from "../apps/desktop/perf/scenes.ts";
import { scaleFromScenes } from "./perf-metrics.mjs";

process.stdout.write(`${JSON.stringify(scaleFromScenes(SCENES, SCENE_NAMES))}\n`);

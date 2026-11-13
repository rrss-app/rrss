import "i18next";
import type { resources, SOURCE_LANGUAGE } from "./index";

declare module "i18next" {
  interface CustomTypeOptions {
    defaultNS: "common";
    resources: (typeof resources)[typeof SOURCE_LANGUAGE];
  }
}

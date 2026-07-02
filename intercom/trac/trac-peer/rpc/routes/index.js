import { v1Routes } from "./v1.js";

export const routes = [
  ...v1Routes.map((r) => ({
    ...r,
    path: `/v1${r.path}`,
  })),
];


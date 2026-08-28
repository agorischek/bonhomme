import { shipping } from "./shipping";

export function quote(): number {
  return shipping() + 1;
}


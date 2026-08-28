import { shipping } from "./pricing";

export function quote(): number {
  return shipping();
}


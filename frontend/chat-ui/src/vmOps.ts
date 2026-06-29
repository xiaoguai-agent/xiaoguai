/**
 * VM-ops assistant detection.
 *
 * The VMware-ops starter card (VmwareStarter) is surfaced in the chat welcome
 * only when the active assistant is the VMware-ops persona. Detection is by
 * persona name so it survives renames of the underlying server/team: a persona
 * whose name mentions "运维" (ops) or "vmware" (case-insensitive) is treated as
 * the VM-ops assistant.
 *
 * Kept tiny + pure so it can be unit-reasoned and reused from anywhere.
 */

/** True when a persona name marks it as the VMware-ops assistant. */
export function isVmOpsPersonaName(name: string | null | undefined): boolean {
  if (!name) return false;
  const lower = name.toLowerCase();
  return lower.includes('vmware') || name.includes('运维');
}

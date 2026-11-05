// Reaching what lives behind the sidebar's account menu.
import { fireEvent, screen, within } from "@testing-library/react";

/** Open the account menu at the foot of the sidebar. */
export function openMenu() {
  fireEvent.click(screen.getByTitle("Account and settings"));
  return within(screen.getByRole("menu", { name: "Account and settings" }));
}

export function closeMenu() {
  fireEvent.keyDown(window, { key: "Escape" });
}

/** Open settings from the account menu at one section. */
export function openSettings(section: string) {
  fireEvent.click(openMenu().getByRole("menuitem", { name: /^Settings/ }));
  const dialog = within(screen.getByRole("dialog", { name: "Settings" }));
  fireEvent.click(within(dialog.getByRole("navigation", { name: "Settings sections" })).getByText(section));
  return dialog;
}

/** The Workspaces `+`: "Add" for its menu, "Add workspace" where it opens a folder at once. */
export const addButton = () => screen.getByTitle(/^Add( workspace)?$/) as HTMLButtonElement;

/** Pick a folder from the Workspaces `+`: through its menu when the Host can clone, else at once. */
export function openProject() {
  fireEvent.click(addButton());
  const menu = screen.queryByRole("menu", { name: "Add" });
  if (menu) fireEvent.click(within(menu).getByRole("menuitem", { name: "Open project" }));
}

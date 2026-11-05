import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it } from "vitest";
import { focusMenu, moveMenuFocus } from "@/panels/ControlCenter";

function Menu() {
  return (
    <div role="menu" aria-label="Actions" tabIndex={-1} onKeyDown={moveMenuFocus}>
      <button role="menuitem" disabled>
        Off
      </button>
      <button role="menuitem">First</button>
      <button role="menuitem">Last</button>
    </div>
  );
}

describe("focusMenu", () => {
  afterEach(cleanup);

  it("puts a keyboard open on the first enabled item", () => {
    render(<Menu />);
    fireEvent.click(document.body, { detail: 0 });
    focusMenu(screen.getByRole("menu"));
    expect(document.activeElement).toBe(screen.getByRole("menuitem", { name: "First" }));
  });

  it("leaves a pointer open on the menu, so no item shows the keyboard ring", () => {
    render(<Menu />);
    fireEvent.click(document.body, { detail: 1 });
    focusMenu(screen.getByRole("menu"));
    expect(document.activeElement).toBe(screen.getByRole("menu"));
  });

  it("still lets the arrows reach the items from a pointer open", () => {
    render(<Menu />);
    fireEvent.click(document.body, { detail: 1 });
    const menu = screen.getByRole("menu");
    focusMenu(menu);
    fireEvent.keyDown(menu, { key: "ArrowDown" });
    expect(document.activeElement).toBe(screen.getByRole("menuitem", { name: "First" }));
    menu.focus();
    fireEvent.keyDown(menu, { key: "ArrowUp" });
    expect(document.activeElement).toBe(screen.getByRole("menuitem", { name: "Last" }));
  });
});

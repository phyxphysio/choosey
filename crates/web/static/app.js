/* Choosey's own JavaScript. Hand-written, no build step, loaded with <script src defer>.
   Everything here is an enhancement: the pages work without it.

   Strict CSP: no inline scripts, styles or eval. Listeners are attached once to `document`
   (event delegation), so they keep working after htmx swaps in new content. */
(function () {
  "use strict";

  /* --- Copy buttons ------------------------------------------------------------------------
     Markup: <input id="x" class="copy" readonly> <button data-copy="x" hidden>Copy</button>
     The button ships with `hidden` so people without JavaScript never see a button that does
     nothing; we reveal it here. */

  function revealCopyButtons(root) {
    if (!root || !root.querySelectorAll) return;
    var buttons = root.querySelectorAll("button[data-copy][hidden]");
    for (var i = 0; i < buttons.length; i++) buttons[i].hidden = false;
  }

  // The script is deferred, so the document has been parsed by the time it runs.
  revealCopyButtons(document);
  // htmx fires htmx:load on each piece of content it swaps in (boosted navigation included).
  document.addEventListener("htmx:load", function (event) {
    revealCopyButtons(event.target);
  });

  /* Old-style copy: select the text and ask the browser to copy the selection. Used when the
     async Clipboard API is missing or refused (e.g. not a secure context). Even if the copy
     itself fails, the text is left selected so the person can copy it by hand. */
  function copyBySelecting(input) {
    input.focus();
    input.select();
    try {
      return document.execCommand("copy");
    } catch (e) {
      return false;
    }
  }

  function copy(input) {
    if (navigator.clipboard && navigator.clipboard.writeText) {
      return navigator.clipboard.writeText(input.value).then(
        function () { return true; },
        function () { return copyBySelecting(input); }
      );
    }
    return Promise.resolve(copyBySelecting(input));
  }

  /* Show "Copied" (or "Selected" if only the selection worked) for a moment, then restore the
     label. The label change is announced because the button has aria-live="polite". */
  function flash(button, text) {
    if (!button.dataset.label) button.dataset.label = button.textContent;
    button.textContent = text;
    button.classList.add("is-copied");
    clearTimeout(Number(button.dataset.timer));
    button.dataset.timer = String(setTimeout(function () {
      button.textContent = button.dataset.label;
      button.classList.remove("is-copied");
    }, 2000));
  }

  document.addEventListener("click", function (event) {
    var button = event.target.closest && event.target.closest("button[data-copy]");
    if (!button) return;
    var input = document.getElementById(button.dataset.copy);
    if (!input) return;
    copy(input).then(function (ok) {
      flash(button, ok ? "Copied" : "Selected");
    });
  });

  /* Clicking or tabbing into a read-only link box selects all of it, ready to copy. Done on
     click as well as focus because some browsers (Safari) clear a selection made on focus when
     the mouse button is released. */
  function selectLink(event) {
    var input = event.target;
    if (input.matches && input.matches("input.copy")) input.select();
  }
  document.addEventListener("focusin", selectLink);
  document.addEventListener("click", selectLink);
})();

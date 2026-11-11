"""What the window shows and what its buttons ask of the app."""

from __future__ import annotations

from .cdp import Page

PHONE_WIDE = {"width": 420, "height": 760, "deviceScaleFactor": 1, "mobile": False}
CALLS = "window.__mock.calls.map(c => c[0])"



def to(page, name):
    """A page of the window, chosen in its sidebar as a person does."""
    page.wait(f"!!document.querySelector('#nav [data-page={name}]')", what="the sidebar")
    page.click(f"#nav [data-page={name}]")

def test_every_folder_with_its_state_conflicts_and_waiting_files(make_page):
    page: Page = make_page()
    assert page.count(".card") == 2
    first = page.eval("document.querySelector('.card').textContent")
    for words in ("report-salmonella", "on lab-server", "Up to date", "Conflicts", "Keep mine", "Waiting to be asked for",
                  "S1_R1.fastq.gz", "3.9 GB", "Notes", "Recent activity (5)"):
        assert words in first, words
    second = page.eval("document.querySelectorAll('.card')[1].textContent")
    assert "Syncing…" in second and "read only" in second
    assert page.eval("document.querySelector('#computer').textContent") == "on lap684"


def test_a_conflict_decided_and_a_big_file_fetched(make_page):
    page = make_page()
    page.click_text("Keep mine")
    page.wait("!document.querySelector('.sect.alert')", what="the conflict gone from the list")
    page.click_text("Fetch")
    page.wait("[...document.querySelectorAll('.card')][0].textContent.includes('Waiting to be asked for1')", what="one left")
    calls = page.eval("JSON.stringify(window.__mock.calls)")
    assert '"resolve",{"id":"a1","copy":"tables/summary (conflict from lap684 2026-10-14 1128).xlsx","choice":"mine"}' in calls
    assert '"fetch",{"id":"a1","path":"reads/S1_R1.fastq.gz"}' in calls


def test_connecting_from_a_pasted_link(make_page):
    page = make_page("?empty")
    assert "No folder connected yet" in page.text("#main")
    page.click("#main input")
    page.type("argus-sync://pair?url=http%3A%2F%2Flab-server%3A8090&key=as_x")
    page.click_text("Continue")
    page.wait("!!document.querySelector('.found')", what="what the link opens, asked of the server")
    assert "wgs-batch-12 on lab-server" in page.text(".found") and "both ways" in page.text(".found")
    assert page.eval("document.querySelector('.box label + .found + label input').value") == "/Users/sam/argus-bridge/wgs-batch-12"
    page.click_text("Choose…", within=".box button")
    page.wait("document.querySelector('.box label + .found + label input').value === '/Users/sam/Desktop/chosen'")
    page.click_text("Connect", within=".box button")
    page.wait("document.querySelectorAll('.card').length === 1 && document.querySelector('#sheet').hidden")
    assert "connect" in page.eval(CALLS)


def test_a_bad_link_says_so_and_cannot_connect(make_page):
    page = make_page("?empty")
    page.click_text("Connect a folder", within="#add")
    page.type("hello")
    page.wait("!document.querySelector('.box .err').hidden", what="the error")
    assert "argus-bridge://" in page.text(".box .err")
    assert page.eval("[...document.querySelectorAll('.box button')].find(b => b.textContent === 'Connect').disabled")


def test_a_link_clicked_while_open_opens_the_sheet(make_page):
    page = make_page()
    page.eval("window.__mock.emit('invite', 'argus-sync://pair?url=http%3A%2F%2Fx&key=as_y')")
    page.wait("!!document.querySelector('.found')")


def test_the_menu_pause_and_disconnect(make_page):
    page = make_page()
    page.click_text("Pause", within=".card button")
    page.wait("document.querySelector('.card .chip.warn')?.textContent === 'Paused'")
    page.click(".card .cardhead button")
    page.click_text("Disconnect…", within=".menu button")
    assert "The files stay where they are" in page.text(".box")
    page.click_text("Disconnect", within=".box button")
    page.wait("document.querySelectorAll('.card').length === 1")


def test_italian_and_light(make_page):
    page = make_page(lang="it-IT", dark=False)
    text = page.eval("document.body.textContent")
    for words in ("Collega una cartella", "Aggiornata", "Tieni la mia", "In attesa di conferma", "su lab-server"):
        assert words in text, words
    assert page.eval("getComputedStyle(document.body).backgroundColor") == "rgb(246, 247, 249)"


def test_narrow_window_has_no_sideways_scroll(make_page):
    page = make_page(viewport=PHONE_WIDE)
    assert page.eval("document.documentElement.scrollWidth <= innerWidth"), "the window scrolls sideways"
    assert page.eval("[...document.querySelectorAll('.card')].every(c => c.getBoundingClientRect().right <= innerWidth)")


def test_open_asks_the_app_for_the_folder_by_id(make_page):
    """Not a path: the app opens only what is inside a synced folder (the opener plugin's own
    scope refused every path — "not allowed to open" on a real Mac)."""
    page = make_page()
    page.click_text("Open", within=".card .where button")
    page.click(".sect.alert a.link")
    calls = page.eval("JSON.stringify(window.__mock.calls.filter(c => c[0] === 'open_in'))")
    assert calls == '[["open_in",{"id":"a1","rel":null}],["open_in",{"id":"a1","rel":"tables/summary (conflict from lap684 2026-10-14 1128).xlsx"}]]', calls
    assert "plugin:opener" not in (page.eval("fetch('app.js').then(r => r.text())"))


def test_a_link_to_connect_this_computer_asks_what_it_lends_all_off(make_page):
    """argus-sync://computer?… opens a sheet of its own: three switches, off until switched on."""
    page = make_page("?empty")
    page.click("#main input")
    page.type("argus-sync://computer?url=http%3A%2F%2Flab-server%3A8090&key=ac_x&name=lap684")
    page.click_text("Continue")
    page.wait("!!document.querySelector('#sheet .switch')", what="the switches")
    assert page.eval("[...document.querySelectorAll('#sheet .switch input')].map(i => i.checked).join()") == "false,false,false"
    assert "lab-server" in page.text("#sheet h2")
    page.click("#sheet .switch[data-offer=\"terminal\"]")
    assert page.eval("document.querySelector('#sheet .switch[data-offer=\"terminal\"] input').checked"), "switched on"
    page.click("#sheet .foot .primary")
    page.wait("!!document.querySelector('.group.machine')", what="this computer's page")
    calls = page.eval("JSON.stringify(window.__mock.calls)")
    assert '"machine_link",{"link":"argus-sync://computer?url=http%3A%2F%2Flab-server%3A8090&key=ac_x&name=lap684","browser":false,"terminal":true,"actions":false}' in calls, calls


def test_this_computers_switches_and_connection(make_page):
    page = make_page("?machine")
    to(page, "computer")
    card = "document.querySelector('.group.machine')"
    page.wait(f"!!{card}", what="this computer")
    head = page.eval("document.querySelector('.pagehead').textContent")
    assert "Connected" in head and "lab-server" in head, head
    assert page.eval(f"[...{card}.querySelectorAll('.switch input')].map(i => i.checked).join()") == "true,true,false"
    page.click(".group.machine .switch[data-offer=\"actions\"] .toggle")
    page.wait("window.__mock.calls.some(c => c[0] === 'machine_set' && c[1].actions === true && c[1].on === true)", what="actions on")
    page.click_text("Disconnect")
    page.wait("window.__mock.calls.some(c => c[0] === 'machine_set' && c[1].on === false)", what="off")


def test_a_newer_version_is_one_press_away(make_page):
    page = make_page("?update")
    bar = "document.querySelector('.updatebar')"
    page.wait(f"!!{bar}", what="the update offered")
    assert "argus-bridge 0.1.2 is ready" in page.eval(f"{bar}.textContent") and "you have 0.1.1" in page.eval(f"{bar}.textContent")
    page.click(".updatebar button")
    page.wait("window.__mock.calls.some(c => c[0] === 'update_now')", what="installed")
    page.wait(f"!{bar}", what="gone once updated")


def test_no_bar_when_this_is_the_newest(make_page):
    page = make_page()
    page.wait("!!document.querySelector('.card')")
    assert page.eval("!document.querySelector('.updatebar')")


def test_argus_opens_in_a_window_of_the_app(make_page):
    page = make_page("?machine")
    page.wait("!document.querySelector('#argus').hidden", what="Open Argus offered")
    assert page.text("#argus") == "Open Argus"
    page.click("#argus")
    page.wait("window.__mock.calls.some(c => c[0] === 'open_argus')", what="asked of the app")


def test_no_argus_to_open_with_nothing_connected(make_page):
    page = make_page("?empty")
    page.wait("!!document.querySelector('#main')")
    assert page.eval("document.querySelector('#argus').hidden")


def test_a_mac_without_full_disk_access_is_told_once_how_to_stop_the_questions(make_page):
    """"It asks Allow every time, and the agent waits" (2026-11-09): with Terminal on and no Full Disk
    Access, the card says why and opens the setting; with it given, or nothing lent that runs commands,
    nothing is said."""
    page = make_page("?machine&nofulldisk")
    to(page, "computer")
    note = "document.querySelector('.fulldisk')"
    page.wait(f"!!{note}", what="the note")
    page.click(".fulldisk button")
    page.wait("window.__mock.calls.some(c => c[0] === 'open_full_disk_settings')", what="the setting opened")
    page = make_page("?machine")
    to(page, "computer")
    page.wait("!!document.querySelector('.group.machine')")
    assert page.eval("!document.querySelector('.fulldisk')")


def test_updates_are_asked_first_and_checked_by_hand(make_page):
    """"A dialog: a new version is out, update? — someone may not want it automatic; and a button to
    check" (2026-11-09). Asked first by default; the switch and the button reach the app."""
    page = make_page()
    to(page, "settings")
    page.wait("!!document.getElementById('checkupdates')", what="the settings")
    assert "argus-bridge 0.1.1" in page.eval("document.querySelector('#main').textContent")
    assert page.eval("document.getElementById('autoupdate').checked") is False
    page.click("#checkupdates")
    page.wait("window.__mock.calls.some(c => c[0] === 'check_updates')", what="checked")
    page.click("#autoupdate")
    page.wait("window.__mock.calls.some(c => c[0] === 'set_auto_update' && c[1].on === true)", what="automatic")


def test_the_language_is_chosen_at_the_foot_of_the_window(make_page):
    """"How is the language changed in argus-bridge?" (2026-11-09): it followed the system's only."""
    page = make_page()
    to(page, "settings")
    page.wait("!!document.getElementById('language')", what="the choice")
    page.eval("(() => { const s = document.getElementById('language'); s.value = 'it'; s.dispatchEvent(new Event('change')); })()")
    page.wait("document.querySelector('#nav [data-page=folders]')?.textContent.includes('Cartelle')", what="in Italian after the reload")
    assert page.eval("document.getElementById('language').value") == "it"


def test_how_the_lent_browser_starts_is_the_persons_to_set(make_page):
    """"Can these be configured, so the user controls the harness?" (2026-11-10): hidden automation (on
    by default), which browser, more switches — saved for the browser's next start."""
    page = make_page("?machine")
    to(page, "computer")
    page.wait("!!document.querySelector('.browserset')", what="the browser's settings")
    assert page.eval("document.querySelector('.browserset input[type=checkbox]').checked") is True
    page.eval("""(() => { const s = document.getElementById('browserexe'); s.value = s.options[2].value; s.dispatchEvent(new Event('change'));
        document.getElementById('browserflags').value = '--lang=it'; document.querySelector('.browserset input[type=checkbox]').click(); })()""")
    page.eval("[...document.querySelectorAll('.browserset button')].find(b => b.textContent.trim() === 'Save').click()")
    page.wait("window.__mock.calls.some(c => c[0] === 'machine_browser' && c[1].hideAutomation === false && c[1].flags === '--lang=it' && c[1].exe.includes('Brave'))", what="saved")


def test_logins_for_agents_are_kept_and_switched_from_the_window(make_page):
    """"Read the passwords from there" (2026-11-10): the switch (off by default), a login added by hand
    straight to the keychain, removed — names and sites only on screen, never a password."""
    page = make_page("?machine")
    to(page, "logins")
    page.wait("!!document.querySelector('.logins')", what="the logins")
    assert page.eval("document.querySelector('.logins input[type=checkbox]').checked") is False
    page.click(".logins label.check")
    page.wait("window.__mock.calls.some(c => c[0] === 'logins_set' && c[1].enabled === true)", what="switched on")
    page.click_text("Add a login…")
    page.eval("""(() => { const v = (id, x) => { const e = document.getElementById(id); e.value = x; };
        v('loginsite', 'portal.example.org'); v('loginuser', 'me@lab'); v('loginpass', 'p4ss'); })()""")
    page.click_text("Keep it")
    page.wait("[...document.querySelectorAll('.item.login')].some(r => r.textContent.includes('portal.example.org'))", what="kept")
    assert "p4ss" not in page.eval("document.body.innerText")
    page.eval("[...document.querySelectorAll('.item.login')].find(r => r.textContent.includes('portal.example.org')).querySelector('button.iconbtn').click()")
    page.wait("window.__mock.calls.some(c => c[0] === 'login_remove' && c[1].site === 'portal.example.org')", what="removed")


def test_a_login_gets_a_code_to_tell_an_agent_which_one(make_page):
    """"Besides the site, a code — so I can tell the agent: use the code XYZ when you sign in; a site may
    have three accounts" (2026-11-11). Given when the login is added, or later from the list; changed, cleared."""
    page = make_page("?machine&logins", dark=True)
    to(page, "logins")
    page.wait("!!document.querySelector('.item.login')", what="the logins")
    page.click_text("Add a login…")
    page.eval("""(() => { const v = (id, x) => { const e = document.getElementById(id); e.value = x; };
        v('loginsite', 'portal.example.org'); v('loginuser', 'me@lab'); v('loginpass', 'p4ss'); v('logincode', 'LAB-2'); })()""")
    page.click_text("Keep it")
    page.wait("window.__mock.calls.some(c => c[0] === 'login_add' && c[1].code === 'LAB-2')", what="kept with its code")
    page.wait("[...document.querySelectorAll('.chip.code.on')].some(c => c.textContent === 'LAB-2')", what="the code on its row")
    # One without a code offers one; pressed, it is given.
    page.click("#main .item.login .chip.code:not(.on)")
    page.wait("!!document.getElementById('logincode')", what="the code's sheet")
    page.eval("document.getElementById('logincode').value = 'WORK'")
    page.click_text("Save")
    page.wait("window.__mock.calls.some(c => c[0] === 'login_code' && c[1].code === 'WORK')", what="given")
    page.wait("[...document.querySelectorAll('.chip.code.on')].some(c => c.textContent === 'WORK')", what="shown")
    # Cleared from the same sheet.
    page.eval("[...document.querySelectorAll('.chip.code.on')].find(c => c.textContent === 'WORK').click()")
    page.click_text("Clear")
    page.wait("window.__mock.calls.some(c => c[0] === 'login_code' && c[1].code === '')", what="cleared")


def test_a_section_opened_stays_open_and_typing_is_not_wiped_by_a_repaint(make_page):
    """"When I open the sections they close again" (2026-11-10): the state arrives every few seconds and
    redrew everything. An opened section stays open; a field being typed in keeps what was typed."""
    page = make_page("?machine")
    page.wait("!!document.querySelector('.card details')", what="a folder's activity")
    page.click(".card details summary")
    page.eval("new Promise((r) => setTimeout(r, 50))")       # the toggle event comes a moment after the click
    page.eval("window.__mock.emit('snapshot', window.__mock.state)")
    assert page.eval("document.querySelector('.card details').open") is True, "still open after a repaint"
    to(page, "computer")
    page.wait("!!document.getElementById('browserflags')", what="the browser's settings")
    page.eval("document.getElementById('browserflags').focus(); document.getElementById('browserflags').value = '--lang=it'")
    page.eval("window.__mock.emit('snapshot', window.__mock.state)")
    assert page.eval("document.getElementById('browserflags').value") == "--lang=it", "typing kept"


def test_a_click_survives_a_repaint_between_press_and_release(make_page):
    """"Open Argus did not open at the first click; the sidebar's clicks do not always work" (2026-11-11). The
    state arrives several times a second while folders sync, and every arrival rebuilt the sidebar and the
    button: pressed on one element, released on its replacement, the click was lost."""
    page = make_page("?machine")
    page.wait("!document.querySelector('#argus').hidden", what="Open Argus offered")
    emit = "window.__mock.emit('snapshot', structuredClone(window.__mock.state))"

    def press_repaint_release(finder):
        x, y = page._center(finder)
        page.send("Input.dispatchMouseEvent", {"type": "mouseMoved", "x": x, "y": y, "button": "none"})
        page.send("Input.dispatchMouseEvent", {"type": "mousePressed", "x": x, "y": y, "button": "left", "clickCount": 1})
        page.eval(emit)
        page.send("Input.dispatchMouseEvent", {"type": "mouseReleased", "x": x, "y": y, "button": "left", "clickCount": 1})

    # The icon inside the button, where a finger or a pointer often lands.
    press_repaint_release("document.querySelector('#argus svg')")
    page.wait("window.__mock.calls.some(c => c[0] === 'open_argus')", timeout=3, what="Open Argus at the first click")
    press_repaint_release("document.querySelector('#nav [data-page=settings] svg')")
    page.wait("document.querySelector('#nav [data-page=settings]').classList.contains('on')", timeout=3, what="the sidebar at the first click")
    # The repaint held during the press still happens afterwards.
    page.eval("window.__mock.state.version = '9.9.9'; " + emit)
    page.wait("document.querySelector('#version').textContent.includes('9.9.9')", timeout=3, what="painted after")

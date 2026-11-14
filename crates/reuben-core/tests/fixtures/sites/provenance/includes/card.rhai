// A card.
fn card(title) {
    raw("<section class='card'><h2>" + escape(title) + "</h2></section>")
}

fn note(text) {
    markdown("*" + text + "*")
}

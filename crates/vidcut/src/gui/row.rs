use eframe::egui;

use super::workspace::Workspace;

pub(super) struct Row<'a, T> {
    pub left: &'a [T],
    pub fill: Option<(T, egui::Align)>,
    pub right: &'a [T],
    pub leaving: &'a [T],
    pub group: fn(T) -> u8,
}

#[derive(Debug, Clone, Copy)]
struct Slot {
    width: f32,
    group: u8,
    right: bool,
}

fn places(slots: &[Slot], gone: &[usize], right: bool) -> Vec<Option<usize>> {
    (0..slots.len())
        .filter(|&k| slots[k].right == right)
        .filter_map(|k| match gone.first() {
            Some(&first) if k == first => Some(None),
            _ if gone.contains(&k) => None,
            _ => Some(Some(k)),
        })
        .collect()
}

fn width(more: f32, sep: f32, slots: &[Slot], gone: &[usize], places: &[Option<usize>]) -> f32 {
    let mut total = 0.0;
    let mut last = None;
    for p in places {
        let s = slots[p.unwrap_or_else(|| gone[0])];
        if last.is_some_and(|g| g != s.group) {
            total += sep;
        }
        last = Some(s.group);
        total += if p.is_some() { s.width } else { more };
    }
    total
}

fn need(more: f32, sep: f32, slots: &[Slot], gone: &[usize]) -> f32 {
    [false, true]
        .into_iter()
        .map(|right| width(more, sep, slots, gone, &places(slots, gone, right)))
        .sum()
}

fn hidden(room: f32, more: f32, sep: f32, slots: &[Slot], leaving: &[usize]) -> usize {
    (0..=leaving.len())
        .find(|&n| need(more, sep, slots, &leaving[..n]) <= room)
        .unwrap_or(leaving.len())
}

fn more_button(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui)) -> egui::Response {
    ui.menu_button(egui_phosphor::regular::DOTS_THREE, add)
        .response
        .on_hover_text("More")
}

struct Run<'a, T> {
    items: &'a [T],
    slots: &'a [Slot],
    first: usize,
}

impl<T: Copy> Run<'_, T> {
    fn draw<F, M>(
        &self,
        ws: &mut Workspace,
        ui: &mut egui::Ui,
        places: impl Iterator<Item = Option<usize>>,
        draw: &mut F,
        menu: &M,
    ) -> Option<u8>
    where
        F: FnMut(&mut Workspace, &mut egui::Ui, T) -> egui::Response,
        M: Fn(&mut Workspace, &mut egui::Ui, &mut F),
    {
        let mut last = None;
        for p in places {
            let group = self.slots[p.unwrap_or(self.first)].group;
            if last.is_some_and(|g| g != group) {
                ui.separator();
            }
            last = Some(group);
            match p {
                Some(k) => {
                    draw(ws, ui, self.items[k]);
                }
                None => menu(ws, ui, draw),
            }
        }
        last
    }
}

impl Workspace {
    pub(super) fn fit_row<T: Copy + PartialEq>(
        &mut self,
        ui: &mut egui::Ui,
        row: Row<T>,
        mut draw: impl FnMut(&mut Self, &mut egui::Ui, T) -> egui::Response,
    ) {
        let gap = ui.spacing().item_spacing.x;
        let items: Vec<T> = row.left.iter().chain(row.right).copied().collect();
        let mut sizer = ui.new_child(egui::UiBuilder::new().sizing_pass().invisible());
        let slots: Vec<Slot> = items
            .iter()
            .enumerate()
            .map(|(k, &i)| Slot {
                width: draw(self, &mut sizer, i).rect.width() + gap,
                group: (row.group)(i),
                right: k >= row.left.len(),
            })
            .collect();
        let sep = sizer.separator().rect.width() + gap;
        let more = more_button(&mut sizer, |_| {}).rect.width() + gap;
        let leaving: Vec<usize> = row
            .leaving
            .iter()
            .filter_map(|l| items.iter().position(|i| i == l))
            .collect();
        let room = ui.available_width();
        let gone = &leaving[..hidden(room, more, sep, &slots, &leaving)];
        let squeezed = need(more, sep, &slots, gone) > room;
        let run = Run {
            items: &items,
            slots: &slots,
            first: gone.first().copied().unwrap_or_default(),
        };
        let id = ui.unique_id();
        let menu = |ws: &mut Self, ui: &mut egui::Ui, draw: &mut _| {
            let scope = egui::UiBuilder::new().scope_id(id.with("more"));
            ui.scope_builder(scope, |ui| {
                more_button(ui, |ui| {
                    let hidden = (0..items.len()).filter(|k| gone.contains(k)).map(Some);
                    run.draw(ws, ui, hidden, draw, &|_, _, _| {});
                })
            });
        };
        let last = run.draw(
            self,
            ui,
            places(&slots, gone, false).into_iter(),
            &mut draw,
            &menu,
        );
        let right = places(&slots, gone, true);
        if let Some((fill, align)) = row.fill {
            let mut room = ui.available_width() - width(more, sep, &slots, gone, &right);
            if last.is_some_and(|g| g != (row.group)(fill)) && room > sep {
                ui.separator();
                room -= sep;
            }
            if room > gap {
                let size = egui::vec2(room - gap, ui.spacing().interact_size.y);
                let layout = match align {
                    egui::Align::Max => egui::Layout::right_to_left(egui::Align::Center),
                    _ => egui::Layout::left_to_right(egui::Align::Center),
                };
                ui.allocate_ui_with_layout(size, layout, |ui| {
                    ui.set_clip_rect(ui.max_rect().intersect(ui.clip_rect()));
                    draw(self, ui, fill)
                });
            }
        }
        if squeezed {
            run.draw(self, ui, right.into_iter(), &mut draw, &menu);
        } else {
            let layout = egui::UiBuilder::new()
                .scope_id(id.with("right"))
                .layout(egui::Layout::right_to_left(egui::Align::Center));
            ui.scope_builder(layout, |ui| {
                run.draw(self, ui, right.into_iter().rev(), &mut draw, &menu);
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Slot, hidden, need};

    fn slot(width: f32, group: u8, right: bool) -> Slot {
        Slot {
            width,
            group,
            right,
        }
    }

    #[test]
    fn a_separator_goes_between_groups() {
        let slots = [slot(30.0, 0, false), slot(30.0, 1, false)];
        assert_eq!(need(10.0, 5.0, &slots, &[]), 65.0);
    }

    #[test]
    fn the_two_sides_are_apart() {
        let slots = [slot(30.0, 0, false), slot(30.0, 1, true)];
        assert_eq!(need(10.0, 5.0, &slots, &[]), 60.0);
    }

    #[test]
    fn the_menu_takes_the_place_and_group_of_the_first_to_go() {
        let slots = [
            slot(30.0, 0, false),
            slot(30.0, 1, false),
            slot(30.0, 1, false),
        ];
        assert_eq!(need(10.0, 5.0, &slots, &[1]), 75.0);
    }

    #[test]
    fn a_row_that_fits_hides_nothing() {
        let slots = [
            slot(30.0, 0, false),
            slot(30.0, 0, false),
            slot(30.0, 0, true),
        ];
        assert_eq!(hidden(100.0, 10.0, 5.0, &slots, &[2, 1]), 0);
    }

    #[test]
    fn a_narrow_row_hides_until_the_rest_and_the_menu_fit() {
        let slots = [
            slot(30.0, 0, false),
            slot(30.0, 0, false),
            slot(30.0, 0, true),
        ];
        assert_eq!(hidden(70.0, 10.0, 5.0, &slots, &[2, 1]), 1);
    }

    #[test]
    fn a_row_too_narrow_for_any_hides_all_that_can_go() {
        let slots = [
            slot(30.0, 0, false),
            slot(30.0, 0, false),
            slot(30.0, 0, true),
        ];
        assert_eq!(hidden(5.0, 10.0, 5.0, &slots, &[2, 1]), 2);
    }
}

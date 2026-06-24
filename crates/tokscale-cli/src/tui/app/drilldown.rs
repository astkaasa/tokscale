use std::collections::BTreeMap;

use crate::tui::data::{ModelUsage, TokenBreakdown};
use crate::tui::drilldown_state::{
    add_tokens, DrilldownState, DrilldownView, ModelDetailKey, PeriodDetailKey, PeriodGranularity,
};
use crate::tui::navigation::{OverviewMode, SortDirection, SortField, Tab};

use super::App;

impl App {
    pub fn is_drilldown_active(&self) -> bool {
        self.drilldown.is_some()
    }

    pub fn drilldown_view(&self) -> Option<&DrilldownView> {
        self.drilldown.as_ref().map(|state| &state.view)
    }

    pub fn open_selected_model_detail(&mut self) {
        let Some(key) = self.selected_model_detail_key() else {
            self.set_status("No model selected");
            return;
        };
        self.open_model_detail(key);
    }

    pub fn open_model_detail(&mut self, key: ModelDetailKey) {
        self.open_drilldown(DrilldownView::Model(key));
    }

    pub fn open_selected_period_detail(&mut self) {
        let selected_date = {
            let daily = self.get_sorted_daily();
            daily.get(self.selected_index).map(|day| day.date)
        };

        if let Some(date) = selected_date {
            self.open_period_detail(PeriodDetailKey::day(date));
        } else {
            self.set_status("No period selected");
        }
    }

    pub fn open_period_detail(&mut self, key: PeriodDetailKey) {
        self.open_drilldown(DrilldownView::Period(key));
    }

    pub(crate) fn open_drilldown(&mut self, view: DrilldownView) {
        let parent = self.drilldown.take().map(|mut active| {
            active.selected_index = self.selected_index;
            active.scroll_offset = self.scroll_offset;
            active.sort_field = self.sort_field;
            active.sort_direction = self.sort_direction;
            Box::new(active)
        });
        let (
            parent_tab,
            parent_selected_index,
            parent_scroll_offset,
            parent_sort_field,
            parent_sort_direction,
        ) = if let Some(active) = parent.as_ref() {
            (
                active.parent_tab,
                active.parent_selected_index,
                active.parent_scroll_offset,
                active.parent_sort_field,
                active.parent_sort_direction,
            )
        } else {
            (
                self.current_tab,
                self.selected_index,
                self.scroll_offset,
                self.sort_field,
                self.sort_direction,
            )
        };
        let (sort_field, sort_direction) = Self::default_sort_for_drilldown(&view);

        self.drilldown = Some(DrilldownState {
            view,
            parent_tab,
            parent_selected_index,
            parent_scroll_offset,
            parent_sort_field,
            parent_sort_direction,
            sort_field,
            sort_direction,
            selected_index: 0,
            scroll_offset: 0,
            parent,
        });
        self.sort_field = sort_field;
        self.sort_direction = sort_direction;
        self.selected_daily_detail_date = None;
        self.selected_index = 0;
        self.scroll_offset = 0;
        self.clear_status();
        self.clamp_selection();
    }

    pub(crate) fn default_sort_for_drilldown(_view: &DrilldownView) -> (SortField, SortDirection) {
        (SortField::Cost, SortDirection::Descending)
    }

    pub fn close_drilldown(&mut self) {
        let Some(state) = self.drilldown.take() else {
            return;
        };

        if let Some(parent) = state.parent {
            let parent = *parent;
            self.selected_index = parent.selected_index;
            self.scroll_offset = parent.scroll_offset;
            self.sort_field = parent.sort_field;
            self.sort_direction = parent.sort_direction;
            self.drilldown = Some(parent);
            self.clear_status();
            self.clamp_selection();
            return;
        }

        self.current_tab = state.parent_tab;
        self.sort_field = state.parent_sort_field;
        self.sort_direction = state.parent_sort_direction;
        let restored_index = match &state.view {
            DrilldownView::Period(key)
                if state.parent_tab == Tab::Timeline
                    && key.granularity == PeriodGranularity::Day =>
            {
                self.get_sorted_daily()
                    .iter()
                    .position(|day| day.date == key.start)
                    .unwrap_or(state.parent_selected_index)
            }
            _ => state.parent_selected_index,
        };

        self.selected_index = restored_index;
        let max_visible = self.max_visible_items.max(1);
        let viewport_still_holds = restored_index >= state.parent_scroll_offset
            && restored_index < state.parent_scroll_offset + max_visible;
        self.scroll_offset = if viewport_still_holds {
            state.parent_scroll_offset
        } else {
            restored_index.saturating_sub(max_visible / 2)
        };
        self.clear_status();
        self.clamp_selection();
    }

    pub(crate) fn selected_model_detail_key(&self) -> Option<ModelDetailKey> {
        match self.current_tab {
            Tab::Overview => self
                .overview_model_detail_keys()
                .get(self.selected_index)
                .cloned(),
            Tab::Models => self
                .get_sorted_models()
                .get(self.selected_index)
                .map(|model| self.model_detail_key_for_usage(model)),
            _ => None,
        }
    }

    pub fn model_detail_key_for_usage(&self, model: &ModelUsage) -> ModelDetailKey {
        let group_by = self.group_by.borrow().clone();
        let label = if group_by == tokscale_core::GroupBy::WorkspaceModel {
            match &model.workspace_label {
                Some(workspace) => format!("{workspace} / {}", model.model),
                None => model.model.clone(),
            }
        } else {
            model.model.clone()
        };

        ModelDetailKey {
            provider: crate::tui::colors::provider_color_key(&model.provider, &model.model),
            model: label,
            color_key: model.model.clone(),
        }
    }

    pub fn overview_model_detail_keys(&self) -> Vec<ModelDetailKey> {
        if self.overview_mode == OverviewMode::All {
            return self
                .get_sorted_models()
                .into_iter()
                .map(|model| self.model_detail_key_for_usage(model))
                .collect();
        }

        let Some(day) = self.today_usage() else {
            return Vec::new();
        };

        let mut candidates: BTreeMap<
            (String, String, String),
            (ModelDetailKey, TokenBreakdown, f64),
        > = BTreeMap::new();
        for source_info in day.source_breakdown.values() {
            for info in source_info.models.values() {
                let provider =
                    crate::tui::colors::provider_color_key(&info.provider, &info.color_key);
                let key = ModelDetailKey {
                    provider: provider.clone(),
                    model: info.display_name.clone(),
                    color_key: info.color_key.clone(),
                };
                let entry = candidates
                    .entry((provider, info.display_name.clone(), info.color_key.clone()))
                    .or_insert_with(|| (key, TokenBreakdown::default(), 0.0));
                add_tokens(&mut entry.1, &info.tokens);
                if info.cost.is_finite() {
                    entry.2 += info.cost;
                }
            }
        }

        let mut candidates = candidates.into_values().collect::<Vec<_>>();
        match (self.sort_field, self.sort_direction) {
            (SortField::Cost, SortDirection::Descending) => candidates
                .sort_by(|a, b| b.2.total_cmp(&a.2).then_with(|| a.0.model.cmp(&b.0.model))),
            (SortField::Cost, SortDirection::Ascending) => candidates
                .sort_by(|a, b| a.2.total_cmp(&b.2).then_with(|| a.0.model.cmp(&b.0.model))),
            (SortField::Tokens, SortDirection::Descending) => candidates.sort_by(|a, b| {
                b.1.total()
                    .cmp(&a.1.total())
                    .then_with(|| a.0.model.cmp(&b.0.model))
            }),
            (SortField::Tokens, SortDirection::Ascending) => candidates.sort_by(|a, b| {
                a.1.total()
                    .cmp(&b.1.total())
                    .then_with(|| a.0.model.cmp(&b.0.model))
            }),
            (SortField::Date, _) => candidates.sort_by(|a, b| a.0.model.cmp(&b.0.model)),
        }

        candidates
            .into_iter()
            .map(|candidate| candidate.0)
            .collect()
    }

    pub fn drilldown_list_len(&self) -> usize {
        match self.drilldown_view() {
            Some(DrilldownView::Model(_)) => self.get_sorted_model_detail_rows().len(),
            Some(DrilldownView::Period(_)) => self.get_sorted_period_detail_rows().len(),
            None => 0,
        }
    }

    pub fn open_selected_drilldown_child(&mut self) {
        match self.drilldown_view().cloned() {
            Some(DrilldownView::Model(_)) => {
                let Some(row) = self
                    .get_sorted_model_detail_rows()
                    .get(self.selected_index)
                    .cloned()
                else {
                    return;
                };
                self.open_period_detail(PeriodDetailKey::day(row.date));
            }
            Some(DrilldownView::Period(_)) => {
                let Some(row) = self
                    .get_sorted_period_detail_rows()
                    .get(self.selected_index)
                    .cloned()
                else {
                    return;
                };
                self.open_model_detail(ModelDetailKey {
                    provider: row.provider,
                    model: row.model,
                    color_key: row.color_key,
                });
            }
            None => {}
        }
    }
}

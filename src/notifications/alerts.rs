use soroban_sdk::{symbol_short, Address, Env};
use crate::notifications::push_service::emit_notification;

pub fn check_low_resources(env: &Env, player: Address, balance: u32, threshold: u32) {
    if balance < threshold {
        emit_notification(env, player, symbol_short!("low_res"));
    }
}

pub fn notify_rare_discovery(env: &Env, player: Address) {
    emit_notification(env, player, symbol_short!("rare_find"));
}

pub fn notify_crafting_complete(env: &Env, player: Address) {
    emit_notification(env, player, symbol_short!("craft_ok"));
}

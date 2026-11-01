//! Pointer gestures (pinch / rotate).

use std::ops::Deref;
use std::sync::Mutex;

use sctk::reexports::client::globals::{BindError, GlobalList};
use sctk::reexports::client::{delegate_dispatch, Connection, Dispatch, QueueHandle};
use wayland_client::Proxy;
use sctk::reexports::protocols::wp::pointer_gestures::zv1::client::{
    zwp_pointer_gesture_pinch_v1::{self, ZwpPointerGesturePinchV1},
    zwp_pointer_gestures_v1::ZwpPointerGesturesV1,
};

use sctk::compositor::SurfaceData;
use sctk::globals::GlobalData;

use crate::event::{TouchPhase, WindowEvent};
use crate::platform_impl::wayland::state::WinitState;
use crate::platform_impl::wayland::{self, DeviceId, WindowId};

/// State associated with a single `ZwpPointerGesturePinchV1` instance.
#[derive(Debug, Default)]
pub struct PinchGestureData {
    inner: Mutex<PinchGestureInner>,
}

#[derive(Debug)]
struct PinchGestureInner {
    window_id: Option<WindowId>,
    last_scale: f64,
}

impl Default for PinchGestureInner {
    fn default() -> Self {
        Self {
            window_id: None,
            last_scale: 1.0,
        }
    }
}

/// Wrapper around the pointer gestures manager.
pub struct PointerGesturesState {
    manager: ZwpPointerGesturesV1,
}

impl PointerGesturesState {
    pub fn new(
        globals: &GlobalList,
        queue_handle: &QueueHandle<WinitState>,
    ) -> Result<Self, BindError> {
        let manager = globals.bind(queue_handle, 1..=3, GlobalData)?;
        Ok(Self { manager })
    }
}

impl Deref for PointerGesturesState {
    type Target = ZwpPointerGesturesV1;

    fn deref(&self) -> &Self::Target {
        &self.manager
    }
}

impl Dispatch<ZwpPointerGesturesV1, GlobalData, WinitState> for PointerGesturesState {
    fn event(
        _state: &mut WinitState,
        _proxy: &ZwpPointerGesturesV1,
        _event: <ZwpPointerGesturesV1 as wayland_client::Proxy>::Event,
        _data: &GlobalData,
        _conn: &Connection,
        _qhandle: &QueueHandle<WinitState>,
    ) {
    }
}

impl Dispatch<ZwpPointerGesturePinchV1, PinchGestureData, WinitState> for PointerGesturesState {
    fn event(
        state: &mut WinitState,
        _proxy: &ZwpPointerGesturePinchV1,
        event: <ZwpPointerGesturePinchV1 as wayland_client::Proxy>::Event,
        data: &PinchGestureData,
        _conn: &Connection,
        _qhandle: &QueueHandle<WinitState>,
    ) {
        let device_id = crate::event::DeviceId(crate::platform_impl::DeviceId::Wayland(DeviceId));
        let mut inner = data.inner.lock().unwrap();

        match event {
            zwp_pointer_gesture_pinch_v1::Event::Begin { surface, .. } => {
                let parent_surface = match surface.data::<SurfaceData>() {
                    Some(d) => d.parent_surface().unwrap_or(&surface),
                    None => &surface,
                };
                let wid = wayland::make_wid(parent_surface);
                inner.window_id = Some(wid);
                inner.last_scale = 1.0;
                state.events_sink.push_window_event(
                    WindowEvent::PinchGesture {
                        device_id,
                        delta: 0.0,
                        phase: TouchPhase::Started,
                    },
                    wid,
                );
            }
            zwp_pointer_gesture_pinch_v1::Event::Update { scale, rotation, .. } => {
                let Some(wid) = inner.window_id else { return };
                let delta = scale - inner.last_scale;
                inner.last_scale = scale;

                if delta.abs() > 1e-6 {
                    state.events_sink.push_window_event(
                        WindowEvent::PinchGesture {
                            device_id,
                            delta,
                            phase: TouchPhase::Moved,
                        },
                        wid,
                    );
                }
                if rotation.abs() > 1e-6 {
                    state.events_sink.push_window_event(
                        WindowEvent::RotationGesture {
                            device_id,
                            delta: rotation as f32,
                            phase: TouchPhase::Moved,
                        },
                        wid,
                    );
                }
            }
            zwp_pointer_gesture_pinch_v1::Event::End { cancelled, .. } => {
                let Some(wid) = inner.window_id.take() else { return };
                inner.last_scale = 1.0;
                let phase = if cancelled != 0 {
                    TouchPhase::Cancelled
                } else {
                    TouchPhase::Ended
                };
                state.events_sink.push_window_event(
                    WindowEvent::PinchGesture {
                        device_id,
                        delta: 0.0,
                        phase,
                    },
                    wid,
                );
            }
            _ => {}
        }
    }
}

delegate_dispatch!(WinitState: [ZwpPointerGesturePinchV1: PinchGestureData] => PointerGesturesState);
delegate_dispatch!(WinitState: [ZwpPointerGesturesV1: GlobalData] => PointerGesturesState);
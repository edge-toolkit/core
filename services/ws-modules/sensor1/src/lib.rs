use std::cell::RefCell;
use std::rc::Rc;

use et_web::{SENSOR_PERMISSION_GRANTED, request_sensor_permission};
use et_ws_wasm_agent::{js_bool_field, js_nested_object, js_number_field, set_textarea_value};
use tracing::info;
use wasm_bindgen::prelude::*;
use web_sys::Event;

const SENSOR_RENDER_INTERVAL_MS: i32 = 150;

#[derive(Clone, Default)]
struct OrientationReadingState {
    alpha: Option<f64>,
    beta: Option<f64>,
    gamma: Option<f64>,
    absolute: Option<bool>,
}

#[derive(Clone, Default)]
struct MotionReadingState {
    acceleration_x: Option<f64>,
    acceleration_y: Option<f64>,
    acceleration_z: Option<f64>,
    acceleration_including_gravity_x: Option<f64>,
    acceleration_including_gravity_y: Option<f64>,
    acceleration_including_gravity_z: Option<f64>,
    rotation_rate_alpha: Option<f64>,
    rotation_rate_beta: Option<f64>,
    rotation_rate_gamma: Option<f64>,
    interval_ms: Option<f64>,
}

#[wasm_bindgen]
pub struct OrientationReading {
    inner: OrientationReadingState,
}

#[wasm_bindgen]
impl OrientationReading {
    pub fn alpha(&self) -> f64 {
        self.inner.alpha.unwrap_or(0.0)
    }

    pub fn beta(&self) -> f64 {
        self.inner.beta.unwrap_or(0.0)
    }

    pub fn gamma(&self) -> f64 {
        self.inner.gamma.unwrap_or(0.0)
    }

    pub fn absolute(&self) -> bool {
        self.inner.absolute.unwrap_or(false)
    }
}

#[wasm_bindgen]
pub struct MotionReading {
    inner: MotionReadingState,
}

#[wasm_bindgen]
impl MotionReading {
    #[wasm_bindgen(js_name = accelerationX)]
    pub fn acceleration_x(&self) -> f64 {
        self.inner.acceleration_x.unwrap_or(0.0)
    }

    #[wasm_bindgen(js_name = accelerationY)]
    pub fn acceleration_y(&self) -> f64 {
        self.inner.acceleration_y.unwrap_or(0.0)
    }

    #[wasm_bindgen(js_name = accelerationZ)]
    pub fn acceleration_z(&self) -> f64 {
        self.inner.acceleration_z.unwrap_or(0.0)
    }

    #[wasm_bindgen(js_name = accelerationIncludingGravityX)]
    pub fn acceleration_including_gravity_x(&self) -> f64 {
        self.inner.acceleration_including_gravity_x.unwrap_or(0.0)
    }

    #[wasm_bindgen(js_name = accelerationIncludingGravityY)]
    pub fn acceleration_including_gravity_y(&self) -> f64 {
        self.inner.acceleration_including_gravity_y.unwrap_or(0.0)
    }

    #[wasm_bindgen(js_name = accelerationIncludingGravityZ)]
    pub fn acceleration_including_gravity_z(&self) -> f64 {
        self.inner.acceleration_including_gravity_z.unwrap_or(0.0)
    }

    #[wasm_bindgen(js_name = rotationRateAlpha)]
    pub fn rotation_rate_alpha(&self) -> f64 {
        self.inner.rotation_rate_alpha.unwrap_or(0.0)
    }

    #[wasm_bindgen(js_name = rotationRateBeta)]
    pub fn rotation_rate_beta(&self) -> f64 {
        self.inner.rotation_rate_beta.unwrap_or(0.0)
    }

    #[wasm_bindgen(js_name = rotationRateGamma)]
    pub fn rotation_rate_gamma(&self) -> f64 {
        self.inner.rotation_rate_gamma.unwrap_or(0.0)
    }

    #[wasm_bindgen(js_name = intervalMs)]
    pub fn interval_ms(&self) -> f64 {
        self.inner.interval_ms.unwrap_or(0.0)
    }
}

#[wasm_bindgen]
pub struct DeviceSensors {
    active: bool,
    orientation_state: Rc<RefCell<Option<OrientationReadingState>>>,
    motion_state: Rc<RefCell<Option<MotionReadingState>>>,
    orientation_listener: Option<Closure<dyn FnMut(Event)>>,
    motion_listener: Option<Closure<dyn FnMut(Event)>>,
}

impl Default for DeviceSensors {
    fn default() -> Self {
        Self::new()
    }
}

#[wasm_bindgen]
impl DeviceSensors {
    #[wasm_bindgen(constructor)]
    pub fn new() -> DeviceSensors {
        DeviceSensors {
            active: false,
            orientation_state: Rc::new(RefCell::new(None)),
            motion_state: Rc::new(RefCell::new(None)),
            orientation_listener: None,
            motion_listener: None,
        }
    }

    pub async fn start(&mut self) -> Result<(), JsValue> {
        if self.active {
            return Ok(());
        }

        let window = web_sys::window().ok_or_else(|| JsValue::from_str("No window available"))?;

        if js_sys::Reflect::get(&window, &JsValue::from_str("DeviceOrientationEvent"))?.is_undefined()
            && js_sys::Reflect::get(&window, &JsValue::from_str("DeviceMotionEvent"))?.is_undefined()
        {
            return Err(JsValue::from_str(
                "Device orientation and motion APIs are not supported in this browser.",
            ));
        }

        let orientation_permission = request_sensor_permission(js_sys::Reflect::get(
            &window,
            &JsValue::from_str("DeviceOrientationEvent"),
        )?)
        .await?;
        let motion_permission =
            request_sensor_permission(js_sys::Reflect::get(&window, &JsValue::from_str("DeviceMotionEvent"))?).await?;

        if orientation_permission != SENSOR_PERMISSION_GRANTED || motion_permission != SENSOR_PERMISSION_GRANTED {
            return Err(JsValue::from_str(&format!(
                "Sensor permission denied (orientation={orientation_permission}, motion={motion_permission})"
            )));
        }

        *self.orientation_state.borrow_mut() = None;
        *self.motion_state.borrow_mut() = None;

        let orientation_state = self.orientation_state.clone();
        let orientation_listener = Closure::wrap(Box::new(move |event: Event| {
            let value: JsValue = event.into();
            *orientation_state.borrow_mut() = Some(OrientationReadingState {
                alpha: js_number_field(&value, "alpha"),
                beta: js_number_field(&value, "beta"),
                gamma: js_number_field(&value, "gamma"),
                absolute: js_bool_field(&value, "absolute"),
            });
        }) as Box<dyn FnMut(Event)>);

        let motion_state = self.motion_state.clone();
        let motion_listener = Closure::wrap(Box::new(move |event: Event| {
            let value: JsValue = event.into();
            let acceleration = js_nested_object(&value, "acceleration");
            let acceleration_including_gravity = js_nested_object(&value, "accelerationIncludingGravity");
            let rotation_rate = js_nested_object(&value, "rotationRate");

            *motion_state.borrow_mut() = Some(MotionReadingState {
                acceleration_x: acceleration.as_ref().and_then(|v| js_number_field(v, "x")),
                acceleration_y: acceleration.as_ref().and_then(|v| js_number_field(v, "y")),
                acceleration_z: acceleration.as_ref().and_then(|v| js_number_field(v, "z")),
                acceleration_including_gravity_x: acceleration_including_gravity
                    .as_ref()
                    .and_then(|v| js_number_field(v, "x")),
                acceleration_including_gravity_y: acceleration_including_gravity
                    .as_ref()
                    .and_then(|v| js_number_field(v, "y")),
                acceleration_including_gravity_z: acceleration_including_gravity
                    .as_ref()
                    .and_then(|v| js_number_field(v, "z")),
                rotation_rate_alpha: rotation_rate.as_ref().and_then(|v| js_number_field(v, "alpha")),
                rotation_rate_beta: rotation_rate.as_ref().and_then(|v| js_number_field(v, "beta")),
                rotation_rate_gamma: rotation_rate.as_ref().and_then(|v| js_number_field(v, "gamma")),
                interval_ms: js_number_field(&value, "interval"),
            });
        }) as Box<dyn FnMut(Event)>);

        let target: &web_sys::EventTarget = window.as_ref();
        target.add_event_listener_with_callback("deviceorientation", orientation_listener.as_ref().unchecked_ref())?;
        target.add_event_listener_with_callback("devicemotion", motion_listener.as_ref().unchecked_ref())?;

        self.orientation_listener = Some(orientation_listener);
        self.motion_listener = Some(motion_listener);
        self.active = true;
        info!("Device sensors started");
        Ok(())
    }

    pub fn stop(&mut self) -> Result<(), JsValue> {
        if !self.active {
            return Ok(());
        }

        let window = web_sys::window().ok_or_else(|| JsValue::from_str("No window available"))?;
        let target: &web_sys::EventTarget = window.as_ref();

        if let Some(listener) = self.orientation_listener.as_ref() {
            target.remove_event_listener_with_callback("deviceorientation", listener.as_ref().unchecked_ref())?;
        }

        if let Some(listener) = self.motion_listener.as_ref() {
            target.remove_event_listener_with_callback("devicemotion", listener.as_ref().unchecked_ref())?;
        }

        self.orientation_listener = None;
        self.motion_listener = None;
        self.active = false;
        info!("Device sensors stopped");
        Ok(())
    }

    #[wasm_bindgen(js_name = isActive)]
    pub fn is_active(&self) -> bool {
        self.active
    }

    #[wasm_bindgen(js_name = hasOrientation)]
    pub fn has_orientation(&self) -> bool {
        self.orientation_state.borrow().is_some()
    }

    #[wasm_bindgen(js_name = hasMotion)]
    pub fn has_motion(&self) -> bool {
        self.motion_state.borrow().is_some()
    }

    #[wasm_bindgen(js_name = orientationSnapshot)]
    pub fn orientation_snapshot(&self) -> Result<OrientationReading, JsValue> {
        self.orientation_state
            .borrow()
            .clone()
            .map(|inner| OrientationReading { inner })
            .ok_or_else(|| JsValue::from_str("No orientation reading available yet"))
    }

    #[wasm_bindgen(js_name = motionSnapshot)]
    pub fn motion_snapshot(&self) -> Result<MotionReading, JsValue> {
        self.motion_state
            .borrow()
            .clone()
            .map(|inner| MotionReading { inner })
            .ok_or_else(|| JsValue::from_str("No motion reading available yet"))
    }
}

struct SensorStreamRuntime {
    sensors: DeviceSensors,
    render_interval_id: i32,
    _render_closure: Closure<dyn FnMut()>,
}

thread_local! {
    static SENSOR_STREAM_RUNTIME: RefCell<Option<SensorStreamRuntime>> = const { RefCell::new(None) };
}

#[wasm_bindgen(start)]
pub fn init() {
    let _ = tracing_wasm::try_set_as_global_default();
    info!("sensor stream workflow module initialized");
}

#[wasm_bindgen]
pub fn is_running() -> bool {
    SENSOR_STREAM_RUNTIME.with(|runtime| runtime.borrow().is_some())
}

#[wasm_bindgen]
pub async fn run() -> Result<(), JsValue> {
    if is_running() {
        return Ok(());
    }

    let mut sensors = DeviceSensors::new();
    set_sensor_status("sensor stream: requesting sensor access")?;
    sensors.start().await?;
    render_sensor_output(&sensors)?;

    let render_closure = Closure::wrap(Box::new(move || {
        SENSOR_STREAM_RUNTIME.with(|runtime| {
            let runtime_ref = runtime.borrow();
            let Some(runtime) = runtime_ref.as_ref() else {
                return;
            };

            let _ = render_sensor_output(&runtime.sensors);
        });
    }) as Box<dyn FnMut()>);

    let window = web_sys::window().ok_or_else(|| JsValue::from_str("No window available"))?;
    let render_interval_id = window.set_interval_with_callback_and_timeout_and_arguments_0(
        render_closure.as_ref().unchecked_ref(),
        SENSOR_RENDER_INTERVAL_MS,
    )?;

    SENSOR_STREAM_RUNTIME.with(|runtime| {
        runtime.borrow_mut().replace(SensorStreamRuntime {
            sensors,
            render_interval_id,
            _render_closure: render_closure,
        });
    });

    let stop_callback = Closure::once_into_js(move || {
        if is_running() {
            let _ = stop();
            let _ = set_sensor_status("sensor stream: finished automatically after 15 seconds");
        }
    });
    window.set_timeout_with_callback_and_timeout_and_arguments_0(stop_callback.unchecked_ref(), 15000)?;

    set_sensor_status("sensor stream: running")?;
    Ok(())
}

#[wasm_bindgen]
pub fn stop() -> Result<(), JsValue> {
    SENSOR_STREAM_RUNTIME.with(|runtime| {
        if let Some(mut runtime) = runtime.borrow_mut().take() {
            if let Some(window) = web_sys::window() {
                window.clear_interval_with_handle(runtime.render_interval_id);
            }
            runtime.sensors.stop()?;
        }

        Ok::<(), JsValue>(())
    })?;

    set_sensor_status("sensor stream: stopped")?;
    set_textarea_value("sensor-output", "Waiting for device sensor data...")
}

fn set_sensor_status(message: &str) -> Result<(), JsValue> {
    set_textarea_value("module-output", message)
}

fn render_sensor_output(sensors: &DeviceSensors) -> Result<(), JsValue> {
    let orientation = if sensors.has_orientation() {
        Some(sensors.orientation_snapshot()?)
    } else {
        None
    };
    let motion = if sensors.has_motion() {
        Some(sensors.motion_snapshot()?)
    } else {
        None
    };

    let mut lines = vec![
        String::from("Device sensor stream"),
        format!(
            "updated: {}",
            String::from(js_sys::Date::new_0().to_locale_time_string("en-US"))
        ),
        String::new(),
        String::from("orientation"),
    ];

    if let Some(orientation) = orientation {
        lines.push(format!("alpha: {}", format_number(orientation.alpha(), 3)));
        lines.push(format!("beta: {}", format_number(orientation.beta(), 3)));
        lines.push(format!("gamma: {}", format_number(orientation.gamma(), 3)));
        lines.push(format!("absolute: {}", orientation.absolute()));
    } else {
        lines.push(String::from("waiting for orientation event..."));
    }

    lines.push(String::new());
    lines.push(String::from("motion"));
    if let Some(motion) = motion {
        lines.push(format!(
            "acceleration: x={} y={} z={}",
            format_number(motion.acceleration_x(), 3),
            format_number(motion.acceleration_y(), 3),
            format_number(motion.acceleration_z(), 3)
        ));
        lines.push(format!(
            "acceleration including gravity: x={} y={} z={}",
            format_number(motion.acceleration_including_gravity_x(), 3),
            format_number(motion.acceleration_including_gravity_y(), 3),
            format_number(motion.acceleration_including_gravity_z(), 3)
        ));
        lines.push(format!(
            "rotation rate: alpha={} beta={} gamma={}",
            format_number(motion.rotation_rate_alpha(), 3),
            format_number(motion.rotation_rate_beta(), 3),
            format_number(motion.rotation_rate_gamma(), 3)
        ));
        lines.push(format!("interval: {} ms", format_number(motion.interval_ms(), 1)));
    } else {
        lines.push(String::from("waiting for motion event..."));
    }

    set_textarea_value("sensor-output", &lines.join("\n"))
}

fn format_number(value: f64, digits: usize) -> String {
    if value.is_finite() {
        format!("{value:.digits$}")
    } else {
        String::from("n/a")
    }
}

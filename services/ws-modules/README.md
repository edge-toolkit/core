# What browser modules can sense

The modules in this directory run in the browser, so they reach the device's sensors, and the devices plugged into or
paired with it, only through the APIs that browser exposes. Those differ by browser and platform, so each sensor and
device type below is grouped by where it works, with the API, the browser versions, and the modules here that use it.
Support data is from [MDN's browser compatibility data][bcd], read on 2026-11-05.

## Every modern browser, on phones and computers

- **Cameras**, front and back, and USB webcams plugged into a computer: `navigator.mediaDevices.getUserMedia`. Used by
  `video1`, `face-detection`, `pyface1`, `pyeye1` and `pydemo1`.
- **Microphone**: `getUserMedia`. Used by `audio1`, `pyspeech1` and `pydemo1`. `speech-recognition` also uses the
  browser's own speech recognition (`SpeechRecognition`), which Chrome and Safari have, and Firefox only on computers.
- **Location**: the Geolocation API. Used by `geolocation`.
- **Motion and orientation**: `DeviceMotionEvent` (accelerometer and gyroscope) and `DeviceOrientationEvent` (how the
  device is tilted and turned). Used by `sensor1` and `har1`. Phones and tablets have the hardware; most computers do
  not.
- **Compass heading**: on every phone, through a different API in each engine -- the `deviceorientationabsolute`
  event in Chrome and Firefox, and `DeviceOrientationEvent.webkitCompassHeading` in Safari. No module reads it yet.
- **Touchscreen, mouse and pen**: Pointer Events everywhere, and Touch Events on phones and tablets, giving taps,
  swipes, timing and drawing. No module records them yet.

## Chrome and other Chromium browsers, on Android and computers

Chrome, Edge, Opera and Samsung Internet share these. Safari has none of them, so neither does any iPhone or iPad
browser built on Safari's engine, and Firefox has only Web Serial.

- **Bluetooth devices**, such as heart-rate monitors: Web Bluetooth. Chrome on Android from 56, and on computers from
  70, though off by default on Linux. Android WebView lacks it, so an Android app that embeds the page in a WebView
  gets none; only a full browser does. Used by `bluetooth`, whose [README](bluetooth/README.md) covers the iPhone
  workaround.
- **USB devices**, generically: WebUSB, from Chrome 61, on Android as well as computers. No module uses it yet.
- **Serial devices**, such as microcontroller boards and instruments that present a serial port over USB: Web Serial.
  Chrome on computers from 89 and on Android from 148, and Firefox on computers from 151. No module uses it yet.
- **Precise motion sensors**: the Generic Sensor API's `Accelerometer`, `Gyroscope`, `LinearAccelerationSensor`,
  `AbsoluteOrientationSensor` and `RelativeOrientationSensor` from Chrome 67, and `GravitySensor` from 91. Unlike the
  motion events above, the page chooses how often to sample. No module uses them yet.
- **Battery**: its charge level, and whether it is charging, through the Battery Status API from Chrome 38. No module
  uses it yet.

## Chrome on Android only

- **NFC tags**: Web NFC (`NDEFReader`), from Chrome 89. Used by `nfc`.

## Chrome on computers only

- **Human-interface devices** over USB or Bluetooth, such as game controllers and foot pedals: WebHID, from Chrome 89.
  No module uses it yet.

## Chrome, switched off by default

- **Ambient light**: `AmbientLightSensor`.
- **Magnetometer**: `Magnetometer`, the raw magnetic field.

Chrome has both from 56, behind a setting that is off by default, so a participant would have to change it. For
direction alone, the compass heading above needs no setting.

## iPhone and iPad

Apple's App Store rules require every browser on an iPhone or iPad to use Safari's engine, WebKit, except that in
the EU and Japan a browser may apply to use another engine. WebKit browsers all have the first group and nothing from
the Chrome groups, and two things differ from other phones:

- **Compass heading** comes from `webkitCompassHeading`, which only Safari's engine has.
- **Motion and orientation** reach the page only after it asks permission with
  `DeviceMotionEvent.requestPermission`, as `har1` does.

The one Chrome-group device an iPhone can reach is Bluetooth, through the Bluefy browser app that the `bluetooth`
module's README describes.

## Not available to any web page

No browser exposes the barometer, the proximity sensor, the phone's own step counter, or heart-rate and blood-oxygen
readings from the phone or a paired watch, and none reaches Apple Health or Google Health Connect. A study that needs
them needs a native app, or an external device the browser can reach through Bluetooth, USB or a serial port.

[bcd]: https://github.com/mdn/browser-compat-data

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

use windows::core::{Interface, PCWSTR};
use windows::Win32::Media::Audio::{
    eConsole, eRender, IAudioSessionControl2, IAudioSessionManager2, IMMDevice,
    IMMDeviceEnumerator, IMMEndpoint, IMMNotificationClient, IMMNotificationClient_Impl,
    MMDeviceEnumerator, DEVICE_STATE, DEVICE_STATE_ACTIVE, EDataFlow, ERole,
};
use windows::Win32::System::Com::{
    CLSCTX_ALL, CLSCTX_INPROC_SERVER, CoCreateInstance,
};
use windows::Win32::UI::Shell::PropertiesSystem::PROPERTYKEY;

/// Shared atomic counters tracking device change notifications.
pub struct DeviceChangeCounters {
    pub default_device_change_count: AtomicU32,
    pub device_state_change_count: AtomicU32,
}

impl DeviceChangeCounters {
    pub fn new() -> Self {
        Self {
            default_device_change_count: AtomicU32::new(0),
            device_state_change_count: AtomicU32::new(0),
        }
    }
}

/// COM object implementing IMMNotificationClient to track audio device changes.
#[windows::core::implement(IMMNotificationClient)]
struct NotificationClientImpl {
    counters: Arc<DeviceChangeCounters>,
}

impl IMMNotificationClient_Impl for NotificationClientImpl_Impl {
    fn OnDefaultDeviceChanged(
        &self,
        flow: EDataFlow,
        role: ERole,
        _pwstrdefaultdeviceid: &PCWSTR,
    ) -> windows::core::Result<()> {
        if flow == eRender && role == eConsole {
            self.counters
                .default_device_change_count
                .fetch_add(1, Ordering::Relaxed);
        }
        Ok(())
    }

    fn OnDeviceStateChanged(
        &self,
        _pwstrdeviceid: &PCWSTR,
        _dwnewstate: DEVICE_STATE,
    ) -> windows::core::Result<()> {
        self.counters
            .device_state_change_count
            .fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    fn OnDeviceAdded(&self, _pwstrdeviceid: &PCWSTR) -> windows::core::Result<()> {
        Ok(())
    }

    fn OnDeviceRemoved(&self, _pwstrdeviceid: &PCWSTR) -> windows::core::Result<()> {
        Ok(())
    }

    fn OnPropertyValueChanged(
        &self,
        _pwstrdeviceid: &PCWSTR,
        _key: &PROPERTYKEY,
    ) -> windows::core::Result<()> {
        Ok(())
    }
}

/// Register a NotificationClient with the system device enumerator.
///
/// Returns the shared counters (for polling change counts) and the COM interface
/// reference (which must be kept alive for notifications to continue).
pub fn register_notification_client(
) -> windows::core::Result<(Arc<DeviceChangeCounters>, IMMNotificationClient)> {
    let counters = Arc::new(DeviceChangeCounters::new());
    let client: IMMNotificationClient = NotificationClientImpl {
        counters: counters.clone(),
    }
    .into();

    unsafe {
        let enumerator: IMMDeviceEnumerator =
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_INPROC_SERVER)?;
        enumerator.RegisterEndpointNotificationCallback(&client)?;
    }

    Ok((counters, client))
}

/// Get a specific audio render device by its endpoint ID string.
///
/// Returns an error if the device is not found, not in an active state,
/// or is not a render (output) device.
pub fn get_preferred_device(endpoint_id: &str) -> windows::core::Result<IMMDevice> {
    unsafe {
        let enumerator: IMMDeviceEnumerator =
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_INPROC_SERVER)?;

        let wide: Vec<u16> = endpoint_id.encode_utf16().chain(std::iter::once(0)).collect();
        let device = enumerator.GetDevice(PCWSTR(wide.as_ptr()))?;

        // Verify device is active.
        let state = device.GetState()?;
        if state != DEVICE_STATE_ACTIVE {
            return Err(windows::core::Error::new(
                windows::core::HRESULT(-1),
                "Device is not in active state",
            ));
        }

        // Verify it is a render device.
        let endpoint: IMMEndpoint = device.cast()?;
        let flow = endpoint.GetDataFlow()?;
        if flow != eRender {
            return Err(windows::core::Error::new(
                windows::core::HRESULT(-1),
                "Device is not a render device",
            ));
        }

        Ok(device)
    }
}

/// Get the default audio render device for the console role.
pub fn get_default_device() -> windows::core::Result<IMMDevice> {
    unsafe {
        let enumerator: IMMDeviceEnumerator =
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_INPROC_SERVER)?;
        enumerator.GetDefaultAudioEndpoint(eRender, eConsole)
    }
}

/// Disable Windows communication ducking on the given device.
///
/// This prevents Windows from automatically lowering volume when it detects
/// a "communication" audio session. Failure is non-fatal.
pub fn disable_communication_ducking(device: &IMMDevice) -> windows::core::Result<()> {
    unsafe {
        let session_manager: IAudioSessionManager2 = device.Activate(CLSCTX_ALL, None)?;
        let session_control = session_manager.GetAudioSessionControl(None, 0)?;
        let session_control2: IAudioSessionControl2 = session_control.cast()?;
        session_control2.SetDuckingPreference(true)?;
    }
    Ok(())
}

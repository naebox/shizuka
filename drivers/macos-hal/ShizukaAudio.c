// SPDX-License-Identifier: MIT
// Copyright (c) 2026 Shizuka Contributors
//
// Clean-room MIT-licensed macOS CoreAudio User-Space HAL Loopback Plug-In (AudioServerPlugIn).
// Publishes "Shizuka Virtual Mic" (48 kHz, 2-channel 32-bit float Output -> Input loopback)
// inside coreaudiod without requiring kernel extensions or disabling SIP.

#include <CoreAudio/AudioServerPlugIn.h>
#include <CoreFoundation/CoreFoundation.h>
#include <mach/mach_time.h>
#include <pthread.h>
#include <stdatomic.h>
#include <stdint.h>
#include <string.h>

#define kSampleRate             48000.0
#define kChannelCount           2U
#define kBytesPerFrame          (kChannelCount * sizeof(Float32))
#define kRingBufferFrames       16384U // ~341ms ring capacity at 48kHz
#define kZeroTimestampFrames    4096U

enum {
    kObjectID_PlugIn        = kAudioObjectPlugInObject, // 1
    kObjectID_Device        = 2,
    kObjectID_Stream_Output = 3,
    kObjectID_Stream_Input  = 4,
};

#define kDeviceUID   CFSTR("app.shizuka.virtualmic.device")
#define kModelUID    CFSTR("app.shizuka.virtualmic.model")
#define kDeviceName  CFSTR("Shizuka Virtual Mic")
#define kMfgName     CFSTR("Shizuka")

static pthread_mutex_t gStateMutex = PTHREAD_MUTEX_INITIALIZER;
static UInt32 gPlugIn_RefCount = 0;
static AudioServerPlugInHostRef gPlugIn_Host = NULL;

static _Atomic UInt32 gDevice_IOClientCount = 0;
static uint64_t gClock_StartHostTime = 0;
static double gHostTicksPerFrame = 0.0;
static uint64_t gClock_Seed = 1;

// Lock-free loopback buffer & writer freshness tracking
static Float32 gRingBuffer[kRingBufferFrames * kChannelCount];
static _Atomic int64_t gLastWrittenSampleEnd = -1;

static void InitHostTimeRatio(void) {
    mach_timebase_info_data_t tb;
    mach_timebase_info(&tb);
    double nanosPerTick = (double)tb.numer / (double)tb.denom;
    double nanosPerFrame = 1000000000.0 / kSampleRate;
    gHostTicksPerFrame = nanosPerFrame / nanosPerTick;
}

static AudioStreamBasicDescription MakeFormat48kStereo(void) {
    AudioStreamBasicDescription fmt;
    memset(&fmt, 0, sizeof(fmt));
    fmt.mSampleRate       = kSampleRate;
    fmt.mFormatID         = kAudioFormatLinearPCM;
    fmt.mFormatFlags      = kAudioFormatFlagIsFloat | kAudioFormatFlagsNativeEndian | kAudioFormatFlagIsPacked;
    fmt.mBytesPerPacket   = kBytesPerFrame;
    fmt.mFramesPerPacket  = 1;
    fmt.mBytesPerFrame    = kBytesPerFrame;
    fmt.mChannelsPerFrame = kChannelCount;
    fmt.mBitsPerChannel   = 32;
    return fmt;
}

// Forward declarations of AudioServerPlugInDriverInterface methods
static HRESULT CV_QueryInterface(void* inDriver, REFIID inUUID, LPVOID* outInterface);
static ULONG   CV_AddRef(void* inDriver);
static ULONG   CV_Release(void* inDriver);
static OSStatus CV_Initialize(AudioServerPlugInDriverRef inDriver, AudioServerPlugInHostRef inHost);
static OSStatus CV_CreateDevice(AudioServerPlugInDriverRef inDriver, CFDictionaryRef inDescription, const AudioServerPlugInClientInfo* inClientInfo, AudioObjectID* outDeviceObjectID);
static OSStatus CV_DestroyDevice(AudioServerPlugInDriverRef inDriver, AudioObjectID inDeviceObjectID);
static OSStatus CV_AddDeviceClient(AudioServerPlugInDriverRef inDriver, AudioObjectID inDeviceObjectID, const AudioServerPlugInClientInfo* inClientInfo);
static OSStatus CV_RemoveDeviceClient(AudioServerPlugInDriverRef inDriver, AudioObjectID inDeviceObjectID, const AudioServerPlugInClientInfo* inClientInfo);
static OSStatus CV_PerformDeviceConfigurationChange(AudioServerPlugInDriverRef inDriver, AudioObjectID inDeviceObjectID, UInt64 inChangeAction, void* inChangeInfo);
static OSStatus CV_AbortDeviceConfigurationChange(AudioServerPlugInDriverRef inDriver, AudioObjectID inDeviceObjectID, UInt64 inChangeAction, void* inChangeInfo);
static Boolean  CV_HasProperty(AudioServerPlugInDriverRef inDriver, AudioObjectID inObjectID, pid_t inClientProcessID, const AudioObjectPropertyAddress* inAddress);
static OSStatus CV_IsPropertySettable(AudioServerPlugInDriverRef inDriver, AudioObjectID inObjectID, pid_t inClientProcessID, const AudioObjectPropertyAddress* inAddress, Boolean* outIsSettable);
static OSStatus CV_GetPropertyDataSize(AudioServerPlugInDriverRef inDriver, AudioObjectID inObjectID, pid_t inClientProcessID, const AudioObjectPropertyAddress* inAddress, UInt32 inQualifierDataSize, const void* inQualifierData, UInt32* outDataSize);
static OSStatus CV_GetPropertyData(AudioServerPlugInDriverRef inDriver, AudioObjectID inObjectID, pid_t inClientProcessID, const AudioObjectPropertyAddress* inAddress, UInt32 inQualifierDataSize, const void* inQualifierData, UInt32 inDataSize, UInt32* outDataSize, void* outData);
static OSStatus CV_SetPropertyData(AudioServerPlugInDriverRef inDriver, AudioObjectID inObjectID, pid_t inClientProcessID, const AudioObjectPropertyAddress* inAddress, UInt32 inQualifierDataSize, const void* inQualifierData, UInt32 inDataSize, const void* inData);
static OSStatus CV_StartIO(AudioServerPlugInDriverRef inDriver, AudioObjectID inDeviceObjectID, UInt32 inClientID);
static OSStatus CV_StopIO(AudioServerPlugInDriverRef inDriver, AudioObjectID inDeviceObjectID, UInt32 inClientID);
static OSStatus CV_GetZeroTimeStamp(AudioServerPlugInDriverRef inDriver, AudioObjectID inDeviceObjectID, UInt32 inClientID, Float64* outSampleTime, UInt64* outHostTime, UInt64* outSeed);
static OSStatus CV_WillDoIOOperation(AudioServerPlugInDriverRef inDriver, AudioObjectID inDeviceObjectID, UInt32 inClientID, UInt32 inOperationID, Boolean* outWillDo, Boolean* outWillDoInPlace);
static OSStatus CV_BeginIOOperation(AudioServerPlugInDriverRef inDriver, AudioObjectID inDeviceObjectID, UInt32 inClientID, UInt32 inOperationID, UInt32 inIOBufferFrameSize, const AudioServerPlugInIOCycleInfo* inIOCycleInfo);
static OSStatus CV_DoIOOperation(AudioServerPlugInDriverRef inDriver, AudioObjectID inDeviceObjectID, AudioObjectID inStreamObjectID, UInt32 inClientID, UInt32 inOperationID, UInt32 inIOBufferFrameSize, const AudioServerPlugInIOCycleInfo* inIOCycleInfo, void* ioMainBuffer, void* ioSecondaryBuffer);
static OSStatus CV_EndIOOperation(AudioServerPlugInDriverRef inDriver, AudioObjectID inDeviceObjectID, UInt32 inClientID, UInt32 inOperationID, UInt32 inIOBufferFrameSize, const AudioServerPlugInIOCycleInfo* inIOCycleInfo);

static AudioServerPlugInDriverInterface gDriverVTable = {
    NULL,
    CV_QueryInterface,
    CV_AddRef,
    CV_Release,
    CV_Initialize,
    CV_CreateDevice,
    CV_DestroyDevice,
    CV_AddDeviceClient,
    CV_RemoveDeviceClient,
    CV_PerformDeviceConfigurationChange,
    CV_AbortDeviceConfigurationChange,
    CV_HasProperty,
    CV_IsPropertySettable,
    CV_GetPropertyDataSize,
    CV_GetPropertyData,
    CV_SetPropertyData,
    CV_StartIO,
    CV_StopIO,
    CV_GetZeroTimeStamp,
    CV_WillDoIOOperation,
    CV_BeginIOOperation,
    CV_DoIOOperation,
    CV_EndIOOperation
};

static AudioServerPlugInDriverInterface* gDriverInterfacePtr = &gDriverVTable;
static AudioServerPlugInDriverRef gDriverRef = &gDriverInterfacePtr;

__attribute__((visibility("default")))
void* ShizukaAudio_Create(CFAllocatorRef inAllocator, CFUUIDRef inRequestedTypeUUID) {
    (void)inAllocator;
    if (!CFEqual(inRequestedTypeUUID, kAudioServerPlugInTypeUUID)) {
        return NULL;
    }
    InitHostTimeRatio();
    return gDriverRef;
}

static HRESULT CV_QueryInterface(void* inDriver, REFIID inUUID, LPVOID* outInterface) {
    if (inDriver != gDriverRef || outInterface == NULL) {
        return kAudioHardwareBadObjectError;
    }
    CFUUIDRef reqUUID = CFUUIDCreateFromUUIDBytes(NULL, inUUID);
    if (reqUUID == NULL) {
        return E_NOINTERFACE;
    }
    bool match = CFEqual(reqUUID, IUnknownUUID) || CFEqual(reqUUID, kAudioServerPlugInDriverInterfaceUUID);
    CFRelease(reqUUID);
    if (!match) {
        return E_NOINTERFACE;
    }
    pthread_mutex_lock(&gStateMutex);
    ++gPlugIn_RefCount;
    pthread_mutex_unlock(&gStateMutex);
    *outInterface = gDriverRef;
    return S_OK;
}

static ULONG CV_AddRef(void* inDriver) {
    if (inDriver != gDriverRef) return 0;
    pthread_mutex_lock(&gStateMutex);
    ULONG rc = ++gPlugIn_RefCount;
    pthread_mutex_unlock(&gStateMutex);
    return rc;
}

static ULONG CV_Release(void* inDriver) {
    if (inDriver != gDriverRef) return 0;
    pthread_mutex_lock(&gStateMutex);
    ULONG rc = gPlugIn_RefCount > 0 ? --gPlugIn_RefCount : 0;
    pthread_mutex_unlock(&gStateMutex);
    return rc;
}

static OSStatus CV_Initialize(AudioServerPlugInDriverRef inDriver, AudioServerPlugInHostRef inHost) {
    if (inDriver != gDriverRef) return kAudioHardwareBadObjectError;
    gPlugIn_Host = inHost;
    InitHostTimeRatio();
    memset(gRingBuffer, 0, sizeof(gRingBuffer));
    return kAudioHardwareNoError;
}

static OSStatus CV_CreateDevice(AudioServerPlugInDriverRef inDriver, CFDictionaryRef inDescription, const AudioServerPlugInClientInfo* inClientInfo, AudioObjectID* outDeviceObjectID) {
    (void)inDriver; (void)inDescription; (void)inClientInfo; (void)outDeviceObjectID;
    return kAudioHardwareUnsupportedOperationError;
}

static OSStatus CV_DestroyDevice(AudioServerPlugInDriverRef inDriver, AudioObjectID inDeviceObjectID) {
    (void)inDriver; (void)inDeviceObjectID;
    return kAudioHardwareUnsupportedOperationError;
}

static OSStatus CV_AddDeviceClient(AudioServerPlugInDriverRef inDriver, AudioObjectID inDeviceObjectID, const AudioServerPlugInClientInfo* inClientInfo) {
    (void)inDriver; (void)inDeviceObjectID; (void)inClientInfo;
    return kAudioHardwareNoError;
}

static OSStatus CV_RemoveDeviceClient(AudioServerPlugInDriverRef inDriver, AudioObjectID inDeviceObjectID, const AudioServerPlugInClientInfo* inClientInfo) {
    (void)inDriver; (void)inDeviceObjectID; (void)inClientInfo;
    return kAudioHardwareNoError;
}

static OSStatus CV_PerformDeviceConfigurationChange(AudioServerPlugInDriverRef inDriver, AudioObjectID inDeviceObjectID, UInt64 inChangeAction, void* inChangeInfo) {
    (void)inDriver; (void)inDeviceObjectID; (void)inChangeAction; (void)inChangeInfo;
    return kAudioHardwareNoError;
}

static OSStatus CV_AbortDeviceConfigurationChange(AudioServerPlugInDriverRef inDriver, AudioObjectID inDeviceObjectID, UInt64 inChangeAction, void* inChangeInfo) {
    (void)inDriver; (void)inDeviceObjectID; (void)inChangeAction; (void)inChangeInfo;
    return kAudioHardwareNoError;
}

static Boolean CV_HasProperty(AudioServerPlugInDriverRef inDriver, AudioObjectID inObjectID, pid_t inClientProcessID, const AudioObjectPropertyAddress* inAddress) {
    (void)inDriver; (void)inClientProcessID;
    if (inAddress == NULL) return false;

    switch (inObjectID) {
        case kObjectID_PlugIn:
            switch (inAddress->mSelector) {
                case kAudioObjectPropertyBaseClass:
                case kAudioObjectPropertyClass:
                case kAudioObjectPropertyOwner:
                case kAudioObjectPropertyManufacturer:
                case kAudioObjectPropertyOwnedObjects:
                case kAudioPlugInPropertyDeviceList:
                case kAudioPlugInPropertyTranslateUIDToDevice:
                case kAudioPlugInPropertyResourceBundle:
                    return true;
            }
            return false;

        case kObjectID_Device:
            switch (inAddress->mSelector) {
                case kAudioObjectPropertyBaseClass:
                case kAudioObjectPropertyClass:
                case kAudioObjectPropertyOwner:
                case kAudioObjectPropertyName:
                case kAudioObjectPropertyManufacturer:
                case kAudioObjectPropertyOwnedObjects:
                case kAudioDevicePropertyDeviceUID:
                case kAudioDevicePropertyModelUID:
                case kAudioDevicePropertyTransportType:
                case kAudioDevicePropertyRelatedDevices:
                case kAudioDevicePropertyClockDomain:
                case kAudioDevicePropertyDeviceIsAlive:
                case kAudioDevicePropertyDeviceIsRunning:
                case kAudioDevicePropertyDeviceCanBeDefaultDevice:
                case kAudioDevicePropertyDeviceCanBeDefaultSystemDevice:
                case kAudioDevicePropertyLatency:
                case kAudioDevicePropertyStreams:
                case kAudioObjectPropertyControlList:
                case kAudioDevicePropertySafetyOffset:
                case kAudioDevicePropertyNominalSampleRate:
                case kAudioDevicePropertyAvailableNominalSampleRates:
                case kAudioDevicePropertyIsHidden:
                case kAudioDevicePropertyZeroTimeStampPeriod:
                case kAudioDevicePropertyPreferredChannelsForStereo:
                    return true;
            }
            return false;

        case kObjectID_Stream_Output:
        case kObjectID_Stream_Input:
            switch (inAddress->mSelector) {
                case kAudioObjectPropertyBaseClass:
                case kAudioObjectPropertyClass:
                case kAudioObjectPropertyOwner:
                case kAudioObjectPropertyOwnedObjects:
                case kAudioStreamPropertyIsActive:
                case kAudioStreamPropertyDirection:
                case kAudioStreamPropertyTerminalType:
                case kAudioStreamPropertyStartingChannel:
                case kAudioStreamPropertyLatency:
                case kAudioStreamPropertyVirtualFormat:
                case kAudioStreamPropertyPhysicalFormat:
                case kAudioStreamPropertyAvailableVirtualFormats:
                case kAudioStreamPropertyAvailablePhysicalFormats:
                    return true;
            }
            return false;
    }
    return false;
}

static OSStatus CV_IsPropertySettable(AudioServerPlugInDriverRef inDriver, AudioObjectID inObjectID, pid_t inClientProcessID, const AudioObjectPropertyAddress* inAddress, Boolean* outIsSettable) {
    (void)inDriver; (void)inObjectID; (void)inClientProcessID; (void)inAddress;
    if (outIsSettable == NULL) return kAudioHardwareIllegalOperationError;
    *outIsSettable = false;
    return kAudioHardwareNoError;
}

static OSStatus CV_GetPropertyDataSize(AudioServerPlugInDriverRef inDriver, AudioObjectID inObjectID, pid_t inClientProcessID, const AudioObjectPropertyAddress* inAddress, UInt32 inQualifierDataSize, const void* inQualifierData, UInt32* outDataSize) {
    (void)inDriver; (void)inClientProcessID; (void)inQualifierDataSize; (void)inQualifierData;
    if (inAddress == NULL || outDataSize == NULL) return kAudioHardwareIllegalOperationError;

    switch (inObjectID) {
        case kObjectID_PlugIn:
            switch (inAddress->mSelector) {
                case kAudioObjectPropertyBaseClass:
                case kAudioObjectPropertyClass:          *outDataSize = sizeof(AudioClassID); return kAudioHardwareNoError;
                case kAudioObjectPropertyOwner:
                case kAudioObjectPropertyOwnedObjects:
                case kAudioPlugInPropertyDeviceList:
                case kAudioPlugInPropertyTranslateUIDToDevice: *outDataSize = sizeof(AudioObjectID); return kAudioHardwareNoError;
                case kAudioObjectPropertyManufacturer:
                case kAudioPlugInPropertyResourceBundle: *outDataSize = sizeof(CFStringRef); return kAudioHardwareNoError;
            }
            break;

        case kObjectID_Device:
            switch (inAddress->mSelector) {
                case kAudioObjectPropertyBaseClass:
                case kAudioObjectPropertyClass:          *outDataSize = sizeof(AudioClassID); return kAudioHardwareNoError;
                case kAudioObjectPropertyOwner:          *outDataSize = sizeof(AudioObjectID); return kAudioHardwareNoError;
                case kAudioObjectPropertyName:
                case kAudioObjectPropertyManufacturer:
                case kAudioDevicePropertyDeviceUID:
                case kAudioDevicePropertyModelUID:       *outDataSize = sizeof(CFStringRef); return kAudioHardwareNoError;
                case kAudioObjectPropertyOwnedObjects:   *outDataSize = 2 * sizeof(AudioObjectID); return kAudioHardwareNoError;
                case kAudioDevicePropertyTransportType:
                case kAudioDevicePropertyClockDomain:
                case kAudioDevicePropertyDeviceIsAlive:
                case kAudioDevicePropertyDeviceIsRunning:
                case kAudioDevicePropertyDeviceCanBeDefaultDevice:
                case kAudioDevicePropertyDeviceCanBeDefaultSystemDevice:
                case kAudioDevicePropertyLatency:
                case kAudioDevicePropertySafetyOffset:
                case kAudioDevicePropertyIsHidden:
                case kAudioDevicePropertyZeroTimeStampPeriod: *outDataSize = sizeof(UInt32); return kAudioHardwareNoError;
                case kAudioDevicePropertyRelatedDevices: *outDataSize = sizeof(AudioObjectID); return kAudioHardwareNoError;
                case kAudioDevicePropertyStreams:
                    if (inAddress->mScope == kAudioObjectPropertyScopeGlobal) *outDataSize = 2 * sizeof(AudioObjectID);
                    else *outDataSize = sizeof(AudioObjectID);
                    return kAudioHardwareNoError;
                case kAudioObjectPropertyControlList:    *outDataSize = 0; return kAudioHardwareNoError;
                case kAudioDevicePropertyNominalSampleRate: *outDataSize = sizeof(Float64); return kAudioHardwareNoError;
                case kAudioDevicePropertyAvailableNominalSampleRates: *outDataSize = sizeof(AudioValueRange); return kAudioHardwareNoError;
                case kAudioDevicePropertyPreferredChannelsForStereo: *outDataSize = 2 * sizeof(UInt32); return kAudioHardwareNoError;
            }
            break;

        case kObjectID_Stream_Output:
        case kObjectID_Stream_Input:
            switch (inAddress->mSelector) {
                case kAudioObjectPropertyBaseClass:
                case kAudioObjectPropertyClass:          *outDataSize = sizeof(AudioClassID); return kAudioHardwareNoError;
                case kAudioObjectPropertyOwner:          *outDataSize = sizeof(AudioObjectID); return kAudioHardwareNoError;
                case kAudioObjectPropertyOwnedObjects:   *outDataSize = 0; return kAudioHardwareNoError;
                case kAudioStreamPropertyIsActive:
                case kAudioStreamPropertyDirection:
                case kAudioStreamPropertyTerminalType:
                case kAudioStreamPropertyStartingChannel:
                case kAudioStreamPropertyLatency:        *outDataSize = sizeof(UInt32); return kAudioHardwareNoError;
                case kAudioStreamPropertyVirtualFormat:
                case kAudioStreamPropertyPhysicalFormat: *outDataSize = sizeof(AudioStreamBasicDescription); return kAudioHardwareNoError;
                case kAudioStreamPropertyAvailableVirtualFormats:
                case kAudioStreamPropertyAvailablePhysicalFormats: *outDataSize = sizeof(AudioStreamRangedDescription); return kAudioHardwareNoError;
            }
            break;
    }
    return kAudioHardwareUnknownPropertyError;
}

static OSStatus CV_GetPropertyData(AudioServerPlugInDriverRef inDriver, AudioObjectID inObjectID, pid_t inClientProcessID, const AudioObjectPropertyAddress* inAddress, UInt32 inQualifierDataSize, const void* inQualifierData, UInt32 inDataSize, UInt32* outDataSize, void* outData) {
    (void)inDriver; (void)inClientProcessID;
    if (inAddress == NULL || outDataSize == NULL || outData == NULL) return kAudioHardwareIllegalOperationError;

    switch (inObjectID) {
        case kObjectID_PlugIn:
            switch (inAddress->mSelector) {
                case kAudioObjectPropertyBaseClass:
                    if (inDataSize < sizeof(AudioClassID)) return kAudioHardwareBadPropertySizeError;
                    *((AudioClassID*)outData) = kAudioObjectClassID;
                    *outDataSize = sizeof(AudioClassID);
                    return kAudioHardwareNoError;
                case kAudioObjectPropertyClass:
                    if (inDataSize < sizeof(AudioClassID)) return kAudioHardwareBadPropertySizeError;
                    *((AudioClassID*)outData) = kAudioPlugInClassID;
                    *outDataSize = sizeof(AudioClassID);
                    return kAudioHardwareNoError;
                case kAudioObjectPropertyOwner:
                    if (inDataSize < sizeof(AudioObjectID)) return kAudioHardwareBadPropertySizeError;
                    *((AudioObjectID*)outData) = kAudioObjectUnknown;
                    *outDataSize = sizeof(AudioObjectID);
                    return kAudioHardwareNoError;
                case kAudioObjectPropertyManufacturer:
                    if (inDataSize < sizeof(CFStringRef)) return kAudioHardwareBadPropertySizeError;
                    *((CFStringRef*)outData) = kMfgName;
                    CFRetain(*((CFStringRef*)outData));
                    *outDataSize = sizeof(CFStringRef);
                    return kAudioHardwareNoError;
                case kAudioObjectPropertyOwnedObjects:
                case kAudioPlugInPropertyDeviceList:
                    if (inDataSize < sizeof(AudioObjectID)) { *outDataSize = 0; return kAudioHardwareNoError; }
                    *((AudioObjectID*)outData) = kObjectID_Device;
                    *outDataSize = sizeof(AudioObjectID);
                    return kAudioHardwareNoError;
                case kAudioPlugInPropertyTranslateUIDToDevice: {
                    if (inDataSize < sizeof(AudioObjectID) || inQualifierDataSize < sizeof(CFStringRef) || inQualifierData == NULL)
                        return kAudioHardwareBadPropertySizeError;
                    CFStringRef uid = *((const CFStringRef*)inQualifierData);
                    *((AudioObjectID*)outData) = (uid != NULL && CFEqual(uid, kDeviceUID)) ? kObjectID_Device : kAudioObjectUnknown;
                    *outDataSize = sizeof(AudioObjectID);
                    return kAudioHardwareNoError;
                }
                case kAudioPlugInPropertyResourceBundle:
                    if (inDataSize < sizeof(CFStringRef)) return kAudioHardwareBadPropertySizeError;
                    *((CFStringRef*)outData) = CFSTR("");
                    *outDataSize = sizeof(CFStringRef);
                    return kAudioHardwareNoError;
            }
            break;

        case kObjectID_Device:
            switch (inAddress->mSelector) {
                case kAudioObjectPropertyBaseClass:
                    if (inDataSize < sizeof(AudioClassID)) return kAudioHardwareBadPropertySizeError;
                    *((AudioClassID*)outData) = kAudioObjectClassID;
                    *outDataSize = sizeof(AudioClassID);
                    return kAudioHardwareNoError;
                case kAudioObjectPropertyClass:
                    if (inDataSize < sizeof(AudioClassID)) return kAudioHardwareBadPropertySizeError;
                    *((AudioClassID*)outData) = kAudioDeviceClassID;
                    *outDataSize = sizeof(AudioClassID);
                    return kAudioHardwareNoError;
                case kAudioObjectPropertyOwner:
                    if (inDataSize < sizeof(AudioObjectID)) return kAudioHardwareBadPropertySizeError;
                    *((AudioObjectID*)outData) = kObjectID_PlugIn;
                    *outDataSize = sizeof(AudioObjectID);
                    return kAudioHardwareNoError;
                case kAudioObjectPropertyName:
                    if (inDataSize < sizeof(CFStringRef)) return kAudioHardwareBadPropertySizeError;
                    *((CFStringRef*)outData) = kDeviceName;
                    CFRetain(*((CFStringRef*)outData));
                    *outDataSize = sizeof(CFStringRef);
                    return kAudioHardwareNoError;
                case kAudioObjectPropertyManufacturer:
                    if (inDataSize < sizeof(CFStringRef)) return kAudioHardwareBadPropertySizeError;
                    *((CFStringRef*)outData) = kMfgName;
                    CFRetain(*((CFStringRef*)outData));
                    *outDataSize = sizeof(CFStringRef);
                    return kAudioHardwareNoError;
                case kAudioDevicePropertyDeviceUID:
                    if (inDataSize < sizeof(CFStringRef)) return kAudioHardwareBadPropertySizeError;
                    *((CFStringRef*)outData) = kDeviceUID;
                    CFRetain(*((CFStringRef*)outData));
                    *outDataSize = sizeof(CFStringRef);
                    return kAudioHardwareNoError;
                case kAudioDevicePropertyModelUID:
                    if (inDataSize < sizeof(CFStringRef)) return kAudioHardwareBadPropertySizeError;
                    *((CFStringRef*)outData) = kModelUID;
                    CFRetain(*((CFStringRef*)outData));
                    *outDataSize = sizeof(CFStringRef);
                    return kAudioHardwareNoError;
                case kAudioDevicePropertyTransportType:
                    if (inDataSize < sizeof(UInt32)) return kAudioHardwareBadPropertySizeError;
                    *((UInt32*)outData) = kAudioDeviceTransportTypeVirtual;
                    *outDataSize = sizeof(UInt32);
                    return kAudioHardwareNoError;
                case kAudioDevicePropertyRelatedDevices:
                    if (inDataSize < sizeof(AudioObjectID)) return kAudioHardwareBadPropertySizeError;
                    *((AudioObjectID*)outData) = kObjectID_Device;
                    *outDataSize = sizeof(AudioObjectID);
                    return kAudioHardwareNoError;
                case kAudioDevicePropertyClockDomain:
                    if (inDataSize < sizeof(UInt32)) return kAudioHardwareBadPropertySizeError;
                    *((UInt32*)outData) = 0;
                    *outDataSize = sizeof(UInt32);
                    return kAudioHardwareNoError;
                case kAudioDevicePropertyDeviceIsAlive:
                    if (inDataSize < sizeof(UInt32)) return kAudioHardwareBadPropertySizeError;
                    *((UInt32*)outData) = 1;
                    *outDataSize = sizeof(UInt32);
                    return kAudioHardwareNoError;
                case kAudioDevicePropertyDeviceIsRunning:
                    if (inDataSize < sizeof(UInt32)) return kAudioHardwareBadPropertySizeError;
                    *((UInt32*)outData) = atomic_load(&gDevice_IOClientCount) > 0 ? 1 : 0;
                    *outDataSize = sizeof(UInt32);
                    return kAudioHardwareNoError;
                case kAudioDevicePropertyDeviceCanBeDefaultDevice:
                    if (inDataSize < sizeof(UInt32)) return kAudioHardwareBadPropertySizeError;
                    // Allow selecting as default mic input, discourage accidental default speaker selection
                    *((UInt32*)outData) = (inAddress->mScope == kAudioObjectPropertyScopeInput) ? 1 : 0;
                    *outDataSize = sizeof(UInt32);
                    return kAudioHardwareNoError;
                case kAudioDevicePropertyDeviceCanBeDefaultSystemDevice:
                    if (inDataSize < sizeof(UInt32)) return kAudioHardwareBadPropertySizeError;
                    *((UInt32*)outData) = 0;
                    *outDataSize = sizeof(UInt32);
                    return kAudioHardwareNoError;
                case kAudioDevicePropertyLatency:
                case kAudioDevicePropertySafetyOffset:
                case kAudioDevicePropertyIsHidden:
                    if (inDataSize < sizeof(UInt32)) return kAudioHardwareBadPropertySizeError;
                    *((UInt32*)outData) = 0;
                    *outDataSize = sizeof(UInt32);
                    return kAudioHardwareNoError;
                case kAudioDevicePropertyZeroTimeStampPeriod:
                    if (inDataSize < sizeof(UInt32)) return kAudioHardwareBadPropertySizeError;
                    *((UInt32*)outData) = kZeroTimestampFrames;
                    *outDataSize = sizeof(UInt32);
                    return kAudioHardwareNoError;
                case kAudioObjectPropertyOwnedObjects:
                case kAudioDevicePropertyStreams: {
                    AudioObjectID* ids = (AudioObjectID*)outData;
                    if (inAddress->mScope == kAudioObjectPropertyScopeOutput) {
                        if (inDataSize < sizeof(AudioObjectID)) { *outDataSize = 0; return kAudioHardwareNoError; }
                        ids[0] = kObjectID_Stream_Output;
                        *outDataSize = sizeof(AudioObjectID);
                    } else if (inAddress->mScope == kAudioObjectPropertyScopeInput) {
                        if (inDataSize < sizeof(AudioObjectID)) { *outDataSize = 0; return kAudioHardwareNoError; }
                        ids[0] = kObjectID_Stream_Input;
                        *outDataSize = sizeof(AudioObjectID);
                    } else {
                        UInt32 count = inDataSize / sizeof(AudioObjectID);
                        if (count >= 1) ids[0] = kObjectID_Stream_Output;
                        if (count >= 2) ids[1] = kObjectID_Stream_Input;
                        *outDataSize = (count >= 2 ? 2 : count) * sizeof(AudioObjectID);
                    }
                    return kAudioHardwareNoError;
                }
                case kAudioObjectPropertyControlList:
                    *outDataSize = 0;
                    return kAudioHardwareNoError;
                case kAudioDevicePropertyNominalSampleRate:
                    if (inDataSize < sizeof(Float64)) return kAudioHardwareBadPropertySizeError;
                    *((Float64*)outData) = kSampleRate;
                    *outDataSize = sizeof(Float64);
                    return kAudioHardwareNoError;
                case kAudioDevicePropertyAvailableNominalSampleRates: {
                    if (inDataSize < sizeof(AudioValueRange)) return kAudioHardwareBadPropertySizeError;
                    AudioValueRange* r = (AudioValueRange*)outData;
                    r->mMinimum = kSampleRate;
                    r->mMaximum = kSampleRate;
                    *outDataSize = sizeof(AudioValueRange);
                    return kAudioHardwareNoError;
                }
                case kAudioDevicePropertyPreferredChannelsForStereo: {
                    if (inDataSize < 2 * sizeof(UInt32)) return kAudioHardwareBadPropertySizeError;
                    UInt32* ch = (UInt32*)outData;
                    ch[0] = 1;
                    ch[1] = 2;
                    *outDataSize = 2 * sizeof(UInt32);
                    return kAudioHardwareNoError;
                }
            }
            break;

        case kObjectID_Stream_Output:
        case kObjectID_Stream_Input:
            switch (inAddress->mSelector) {
                case kAudioObjectPropertyBaseClass:
                    if (inDataSize < sizeof(AudioClassID)) return kAudioHardwareBadPropertySizeError;
                    *((AudioClassID*)outData) = kAudioObjectClassID;
                    *outDataSize = sizeof(AudioClassID);
                    return kAudioHardwareNoError;
                case kAudioObjectPropertyClass:
                    if (inDataSize < sizeof(AudioClassID)) return kAudioHardwareBadPropertySizeError;
                    *((AudioClassID*)outData) = kAudioStreamClassID;
                    *outDataSize = sizeof(AudioClassID);
                    return kAudioHardwareNoError;
                case kAudioObjectPropertyOwner:
                    if (inDataSize < sizeof(AudioObjectID)) return kAudioHardwareBadPropertySizeError;
                    *((AudioObjectID*)outData) = kObjectID_Device;
                    *outDataSize = sizeof(AudioObjectID);
                    return kAudioHardwareNoError;
                case kAudioObjectPropertyOwnedObjects:
                    *outDataSize = 0;
                    return kAudioHardwareNoError;
                case kAudioStreamPropertyIsActive:
                    if (inDataSize < sizeof(UInt32)) return kAudioHardwareBadPropertySizeError;
                    *((UInt32*)outData) = 1;
                    *outDataSize = sizeof(UInt32);
                    return kAudioHardwareNoError;
                case kAudioStreamPropertyDirection:
                    if (inDataSize < sizeof(UInt32)) return kAudioHardwareBadPropertySizeError;
                    *((UInt32*)outData) = (inObjectID == kObjectID_Stream_Output) ? 0 : 1;
                    *outDataSize = sizeof(UInt32);
                    return kAudioHardwareNoError;
                case kAudioStreamPropertyTerminalType:
                    if (inDataSize < sizeof(UInt32)) return kAudioHardwareBadPropertySizeError;
                    *((UInt32*)outData) = (inObjectID == kObjectID_Stream_Output) ? kAudioStreamTerminalTypeSpeaker : kAudioStreamTerminalTypeMicrophone;
                    *outDataSize = sizeof(UInt32);
                    return kAudioHardwareNoError;
                case kAudioStreamPropertyStartingChannel:
                    if (inDataSize < sizeof(UInt32)) return kAudioHardwareBadPropertySizeError;
                    *((UInt32*)outData) = 1;
                    *outDataSize = sizeof(UInt32);
                    return kAudioHardwareNoError;
                case kAudioStreamPropertyLatency:
                    if (inDataSize < sizeof(UInt32)) return kAudioHardwareBadPropertySizeError;
                    *((UInt32*)outData) = 0;
                    *outDataSize = sizeof(UInt32);
                    return kAudioHardwareNoError;
                case kAudioStreamPropertyVirtualFormat:
                case kAudioStreamPropertyPhysicalFormat:
                    if (inDataSize < sizeof(AudioStreamBasicDescription)) return kAudioHardwareBadPropertySizeError;
                    *((AudioStreamBasicDescription*)outData) = MakeFormat48kStereo();
                    *outDataSize = sizeof(AudioStreamBasicDescription);
                    return kAudioHardwareNoError;
                case kAudioStreamPropertyAvailableVirtualFormats:
                case kAudioStreamPropertyAvailablePhysicalFormats: {
                    if (inDataSize < sizeof(AudioStreamRangedDescription)) return kAudioHardwareBadPropertySizeError;
                    AudioStreamRangedDescription* rd = (AudioStreamRangedDescription*)outData;
                    rd->mFormat = MakeFormat48kStereo();
                    rd->mSampleRateRange.mMinimum = kSampleRate;
                    rd->mSampleRateRange.mMaximum = kSampleRate;
                    *outDataSize = sizeof(AudioStreamRangedDescription);
                    return kAudioHardwareNoError;
                }
            }
            break;
    }
    return kAudioHardwareUnknownPropertyError;
}

static OSStatus CV_SetPropertyData(AudioServerPlugInDriverRef inDriver, AudioObjectID inObjectID, pid_t inClientProcessID, const AudioObjectPropertyAddress* inAddress, UInt32 inQualifierDataSize, const void* inQualifierData, UInt32 inDataSize, const void* inData) {
    (void)inDriver; (void)inObjectID; (void)inClientProcessID; (void)inAddress;
    (void)inQualifierDataSize; (void)inQualifierData; (void)inDataSize; (void)inData;
    return kAudioHardwareUnsupportedOperationError;
}

static OSStatus CV_StartIO(AudioServerPlugInDriverRef inDriver, AudioObjectID inDeviceObjectID, UInt32 inClientID) {
    (void)inDriver; (void)inClientID;
    if (inDeviceObjectID != kObjectID_Device) return kAudioHardwareBadObjectError;

    pthread_mutex_lock(&gStateMutex);
    UInt32 prev = atomic_fetch_add(&gDevice_IOClientCount, 1);
    if (prev == 0) {
        memset(gRingBuffer, 0, sizeof(gRingBuffer));
        atomic_store(&gLastWrittenSampleEnd, -1);
        gClock_StartHostTime = mach_absolute_time();
        ++gClock_Seed;
    }
    pthread_mutex_unlock(&gStateMutex);
    return kAudioHardwareNoError;
}

static OSStatus CV_StopIO(AudioServerPlugInDriverRef inDriver, AudioObjectID inDeviceObjectID, UInt32 inClientID) {
    (void)inDriver; (void)inClientID;
    if (inDeviceObjectID != kObjectID_Device) return kAudioHardwareBadObjectError;

    pthread_mutex_lock(&gStateMutex);
    UInt32 cur = atomic_load(&gDevice_IOClientCount);
    if (cur > 0) {
        if (atomic_fetch_sub(&gDevice_IOClientCount, 1) == 1) {
            memset(gRingBuffer, 0, sizeof(gRingBuffer));
            atomic_store(&gLastWrittenSampleEnd, -1);
        }
    }
    pthread_mutex_unlock(&gStateMutex);
    return kAudioHardwareNoError;
}

static OSStatus CV_GetZeroTimeStamp(AudioServerPlugInDriverRef inDriver, AudioObjectID inDeviceObjectID, UInt32 inClientID, Float64* outSampleTime, UInt64* outHostTime, UInt64* outSeed) {
    (void)inDriver; (void)inClientID;
    if (inDeviceObjectID != kObjectID_Device) return kAudioHardwareBadObjectError;

    uint64_t now = mach_absolute_time();
    double ticksPerRing = gHostTicksPerFrame * (double)kZeroTimestampFrames;
    uint64_t elapsedTicks = now >= gClock_StartHostTime ? (now - gClock_StartHostTime) : 0;
    uint64_t periods = (uint64_t)((double)elapsedTicks / ticksPerRing);

    *outSampleTime = (Float64)(periods * kZeroTimestampFrames);
    *outHostTime   = gClock_StartHostTime + (uint64_t)((double)periods * ticksPerRing);
    *outSeed       = gClock_Seed;
    return kAudioHardwareNoError;
}

static OSStatus CV_WillDoIOOperation(AudioServerPlugInDriverRef inDriver, AudioObjectID inDeviceObjectID, UInt32 inClientID, UInt32 inOperationID, Boolean* outWillDo, Boolean* outWillDoInPlace) {
    (void)inDriver; (void)inClientID;
    if (inDeviceObjectID != kObjectID_Device) return kAudioHardwareBadObjectError;

    bool supported = (inOperationID == kAudioServerPlugInIOOperationWriteMix) ||
                     (inOperationID == kAudioServerPlugInIOOperationReadInput);
    *outWillDo = supported;
    *outWillDoInPlace = true;
    return kAudioHardwareNoError;
}

static OSStatus CV_BeginIOOperation(AudioServerPlugInDriverRef inDriver, AudioObjectID inDeviceObjectID, UInt32 inClientID, UInt32 inOperationID, UInt32 inIOBufferFrameSize, const AudioServerPlugInIOCycleInfo* inIOCycleInfo) {
    (void)inDriver; (void)inDeviceObjectID; (void)inClientID; (void)inOperationID; (void)inIOBufferFrameSize; (void)inIOCycleInfo;
    return kAudioHardwareNoError;
}

static OSStatus CV_DoIOOperation(AudioServerPlugInDriverRef inDriver, AudioObjectID inDeviceObjectID, AudioObjectID inStreamObjectID, UInt32 inClientID, UInt32 inOperationID, UInt32 inIOBufferFrameSize, const AudioServerPlugInIOCycleInfo* inIOCycleInfo, void* ioMainBuffer, void* ioSecondaryBuffer) {
    (void)inDriver; (void)inDeviceObjectID; (void)inClientID; (void)ioSecondaryBuffer;
    if (ioMainBuffer == NULL || inIOCycleInfo == NULL) return kAudioHardwareNoError;

    Float32* buf = (Float32*)ioMainBuffer;

    if (inOperationID == kAudioServerPlugInIOOperationWriteMix && inStreamObjectID == kObjectID_Stream_Output) {
        int64_t startFrame = (int64_t)inIOCycleInfo->mOutputTime.mSampleTime;
        for (UInt32 f = 0; f < inIOBufferFrameSize; ++f) {
            UInt32 idx = (UInt32)((uint64_t)(startFrame + f) % kRingBufferFrames) * kChannelCount;
            gRingBuffer[idx]     = buf[f * kChannelCount];
            gRingBuffer[idx + 1] = buf[f * kChannelCount + 1];
        }
        atomic_store_explicit(&gLastWrittenSampleEnd, startFrame + (int64_t)inIOBufferFrameSize, memory_order_release);
    } else if (inOperationID == kAudioServerPlugInIOOperationReadInput && inStreamObjectID == kObjectID_Stream_Input) {
        int64_t readStart = (int64_t)inIOCycleInfo->mInputTime.mSampleTime;
        int64_t lastWritten = atomic_load_explicit(&gLastWrittenSampleEnd, memory_order_acquire);

        // If no active writer has written recent frames (e.g. Shizuka app stopped), output clean silence
        if (lastWritten < 0 || readStart > lastWritten + (int64_t)(kRingBufferFrames / 4)) {
            memset(buf, 0, inIOBufferFrameSize * kBytesPerFrame);
        } else {
            for (UInt32 f = 0; f < inIOBufferFrameSize; ++f) {
                UInt32 idx = (UInt32)((uint64_t)(readStart + f) % kRingBufferFrames) * kChannelCount;
                buf[f * kChannelCount]     = gRingBuffer[idx];
                buf[f * kChannelCount + 1] = gRingBuffer[idx + 1];
            }
        }
    }
    return kAudioHardwareNoError;
}

static OSStatus CV_EndIOOperation(AudioServerPlugInDriverRef inDriver, AudioObjectID inDeviceObjectID, UInt32 inClientID, UInt32 inOperationID, UInt32 inIOBufferFrameSize, const AudioServerPlugInIOCycleInfo* inIOCycleInfo) {
    (void)inDriver; (void)inDeviceObjectID; (void)inClientID; (void)inOperationID; (void)inIOBufferFrameSize; (void)inIOCycleInfo;
    return kAudioHardwareNoError;
}

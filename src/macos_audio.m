#import <AppKit/AppKit.h>
#import <CoreMedia/CoreMedia.h>
#import <ScreenCaptureKit/ScreenCaptureKit.h>

#include <alloca.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>

typedef void (*RainbowaveSamplesCallback)(void *context, const float *samples, size_t sample_count);

API_AVAILABLE(macos(14.0))
@interface RainbowaveScreenAudioCapture : NSObject <SCContentSharingPickerObserver, SCStreamDelegate, SCStreamOutput> {
    float *_monoSamples;
    size_t _monoCapacity;
}

@property(nonatomic, assign) RainbowaveSamplesCallback callback;
@property(nonatomic, assign) void *callbackContext;
@property(nonatomic, strong) SCStream *stream;
@property(nonatomic, strong) dispatch_queue_t audioQueue;
@property(nonatomic, copy) NSString *startupError;
@property(nonatomic, assign) BOOL startupFinished;
@property(nonatomic, assign) BOOL running;

- (BOOL)startWithError:(char **)errorOut;
- (void)stopSynchronously;
@end

static char *RainbowaveCopyString(NSString *message) {
    const char *utf8 = message.UTF8String;
    return strdup(utf8 != NULL ? utf8 : "unknown ScreenCaptureKit error");
}

@implementation RainbowaveScreenAudioCapture

- (instancetype)init {
    self = [super init];
    if (self != nil) {
        _audioQueue = dispatch_queue_create("com.yummydirtx.rainbowave.audio", DISPATCH_QUEUE_SERIAL);
    }
    return self;
}

- (void)dealloc {
    free(_monoSamples);
}

- (BOOL)startWithError:(char **)errorOut {
    NSApplication *application = NSApplication.sharedApplication;
    [application setActivationPolicy:NSApplicationActivationPolicyAccessory];
    [application finishLaunching];
    [application activateIgnoringOtherApps:YES];

    SCContentSharingPicker *picker = SCContentSharingPicker.sharedPicker;
    SCContentSharingPickerConfiguration *configuration =
        [[SCContentSharingPickerConfiguration alloc] init];
    configuration.allowedPickerModes = SCContentSharingPickerModeSingleDisplay |
        SCContentSharingPickerModeSingleApplication |
        SCContentSharingPickerModeMultipleApplications;
    configuration.allowsChangingSelectedContent = YES;
    picker.defaultConfiguration = configuration;
    picker.maximumStreamCount = @1;
    [picker addObserver:self];
    picker.active = YES;
    [picker presentPickerUsingContentStyle:SCShareableContentStyleDisplay];

    while (!self.startupFinished) {
        @autoreleasepool {
            [NSRunLoop.currentRunLoop runMode:NSDefaultRunLoopMode
                                   beforeDate:[NSDate dateWithTimeIntervalSinceNow:0.025]];
        }
    }

    if (!self.running) {
        picker.active = NO;
        [picker removeObserver:self];
        if (errorOut != NULL) {
            *errorOut = RainbowaveCopyString(
                self.startupError ?: @"audio source selection was cancelled");
        }
        return NO;
    }

    return YES;
}

- (void)contentSharingPicker:(SCContentSharingPicker *)picker
          didCancelForStream:(SCStream *)stream {
    if (!self.running) {
        self.startupError = @"audio source selection was cancelled";
        self.startupFinished = YES;
    }
}

- (void)contentSharingPicker:(SCContentSharingPicker *)picker
         didUpdateWithFilter:(SCContentFilter *)filter
                   forStream:(SCStream *)stream {
    if (self.running && stream == self.stream) {
        [stream updateContentFilter:filter completionHandler:nil];
        return;
    }

    SCStreamConfiguration *configuration = [[SCStreamConfiguration alloc] init];
    configuration.capturesAudio = YES;
    configuration.excludesCurrentProcessAudio = YES;
    configuration.sampleRate = 48000;
    configuration.channelCount = 2;
    configuration.width = 2;
    configuration.height = 2;
    configuration.minimumFrameInterval = CMTimeMake(1, 1);
    configuration.queueDepth = 1;
    configuration.showsCursor = NO;
    configuration.streamName = @"Rainbowave audio visualization";

    SCStream *newStream = [[SCStream alloc] initWithFilter:filter
                                             configuration:configuration
                                                  delegate:self];
    NSError *outputError = nil;
    if (![newStream addStreamOutput:self
                               type:SCStreamOutputTypeAudio
                 sampleHandlerQueue:self.audioQueue
                              error:&outputError]) {
        self.startupError = [NSString stringWithFormat:@"could not receive shared audio: %@",
                                                       outputError.localizedDescription];
        self.startupFinished = YES;
        return;
    }

    self.stream = newStream;
    [newStream startCaptureWithCompletionHandler:^(NSError *error) {
        if (error != nil) {
            self.startupError = [NSString stringWithFormat:@"could not start shared audio: %@",
                                                           error.localizedDescription];
            self.running = NO;
        } else {
            self.running = YES;
        }
        self.startupFinished = YES;
    }];
}

- (void)contentSharingPickerStartDidFailWithError:(NSError *)error {
    self.startupError = [NSString stringWithFormat:@"could not open the audio sharing picker: %@",
                                                   error.localizedDescription];
    self.startupFinished = YES;
}

- (void)stream:(SCStream *)stream didStopWithError:(NSError *)error {
    self.running = NO;
}

- (void)stream:(SCStream *)stream
    didOutputSampleBuffer:(CMSampleBufferRef)sampleBuffer
                   ofType:(SCStreamOutputType)type {
    if (type != SCStreamOutputTypeAudio || !CMSampleBufferDataIsReady(sampleBuffer)) {
        return;
    }

    CMAudioFormatDescriptionRef format = CMSampleBufferGetFormatDescription(sampleBuffer);
    const AudioStreamBasicDescription *description =
        format == NULL ? NULL : CMAudioFormatDescriptionGetStreamBasicDescription(format);
    if (description == NULL || description->mFormatID != kAudioFormatLinearPCM ||
        (description->mFormatFlags & kAudioFormatFlagIsFloat) == 0 ||
        description->mBitsPerChannel != 32) {
        return;
    }

    size_t bufferListSize = 0;
    OSStatus status = CMSampleBufferGetAudioBufferListWithRetainedBlockBuffer(
        sampleBuffer, &bufferListSize, NULL, 0, NULL, NULL, 0, NULL);
    if (status != noErr || bufferListSize == 0) {
        return;
    }

    AudioBufferList *bufferList = alloca(bufferListSize);
    CMBlockBufferRef retainedBuffer = NULL;
    status = CMSampleBufferGetAudioBufferListWithRetainedBlockBuffer(
        sampleBuffer, NULL, bufferList, bufferListSize, NULL, NULL,
        kCMSampleBufferFlag_AudioBufferList_Assure16ByteAlignment, &retainedBuffer);
    if (status != noErr) {
        return;
    }

    CMItemCount frameCountValue = CMSampleBufferGetNumSamples(sampleBuffer);
    if (frameCountValue <= 0) {
        if (retainedBuffer != NULL) {
            CFRelease(retainedBuffer);
        }
        return;
    }
    size_t frameCount = (size_t)frameCountValue;
    if (_monoCapacity < frameCount) {
        float *resized = realloc(_monoSamples, frameCount * sizeof(float));
        if (resized == NULL) {
            if (retainedBuffer != NULL) {
                CFRelease(retainedBuffer);
            }
            return;
        }
        _monoSamples = resized;
        _monoCapacity = frameCount;
    }

    uint32_t totalChannels = 0;
    for (uint32_t bufferIndex = 0; bufferIndex < bufferList->mNumberBuffers; bufferIndex++) {
        totalChannels += bufferList->mBuffers[bufferIndex].mNumberChannels;
    }
    if (totalChannels == 0) {
        if (retainedBuffer != NULL) {
            CFRelease(retainedBuffer);
        }
        return;
    }

    for (size_t frame = 0; frame < frameCount; frame++) {
        float sum = 0.0f;
        for (uint32_t bufferIndex = 0; bufferIndex < bufferList->mNumberBuffers; bufferIndex++) {
            AudioBuffer buffer = bufferList->mBuffers[bufferIndex];
            const float *samples = (const float *)buffer.mData;
            for (uint32_t channel = 0; channel < buffer.mNumberChannels; channel++) {
                sum += samples[frame * buffer.mNumberChannels + channel];
            }
        }
        _monoSamples[frame] = sum / (float)totalChannels;
    }

    self.callback(self.callbackContext, _monoSamples, frameCount);
    if (retainedBuffer != NULL) {
        CFRelease(retainedBuffer);
    }
}

- (void)stopSynchronously {
    SCContentSharingPicker *picker = SCContentSharingPicker.sharedPicker;
    picker.active = NO;
    [picker removeObserver:self];

    SCStream *stream = self.stream;
    if (stream != nil) {
        dispatch_semaphore_t stopped = dispatch_semaphore_create(0);
        [stream stopCaptureWithCompletionHandler:^(NSError *error) {
            dispatch_semaphore_signal(stopped);
        }];
        while (dispatch_semaphore_wait(stopped, DISPATCH_TIME_NOW) != 0) {
            [NSRunLoop.currentRunLoop runMode:NSDefaultRunLoopMode
                                   beforeDate:[NSDate dateWithTimeIntervalSinceNow:0.01]];
        }
        dispatch_sync(self.audioQueue, ^{});
    }
    self.stream = nil;
    self.running = NO;
}

@end

void *rainbowave_screen_audio_start(RainbowaveSamplesCallback callback,
                                    void *context,
                                    char **errorOut) {
    @autoreleasepool {
        if (errorOut != NULL) {
            *errorOut = NULL;
        }
        RainbowaveScreenAudioCapture *capture = [[RainbowaveScreenAudioCapture alloc] init];
        capture.callback = callback;
        capture.callbackContext = context;
        if (![capture startWithError:errorOut]) {
            return NULL;
        }
        return (__bridge_retained void *)capture;
    }
}

void rainbowave_screen_audio_stop(void *handle) {
    if (handle == NULL) {
        return;
    }
    RainbowaveScreenAudioCapture *capture = (__bridge_transfer RainbowaveScreenAudioCapture *)handle;
    [capture stopSynchronously];
}

void rainbowave_screen_audio_error_free(char *error) {
    free(error);
}

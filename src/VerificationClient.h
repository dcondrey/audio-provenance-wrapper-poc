#pragma once

#include "SafeJson.h"

#include <juce_core/juce_core.h>
#include <juce_cryptography/juce_cryptography.h>

#include <atomic>
#include <cstdint>
#include <cstring>
#include <memory>

namespace apw
{

// IMPORTANT: the verification request/response uses its own port and socket. The
// event port (9876) rejects any packet failing the evidence schema, and a
// rejection turns the daemon dashboard red and degrades the receipt proof level.
//
// IMPORTANT: this plug-in has no signature verifier. It hashes bytes and reads
// local manifests, so it may report a byte binding and nothing stronger.
// Outcome::verified is assigned in parseResponse alone, from a verifier that did
// check signatures; verifyFromLocalEvidence must never reach it.
class VerificationClient final : private juce::Thread
{
public:
    enum class Outcome
    {
        idle,
        inProgress,
        verified,
        boundToLocalManifest,
        changed,
        claimUntrusted,
        claimNotChecked,
        notFound,
        failed
    };

    struct Result
    {
        Outcome outcome = Outcome::idle;
        juce::String fileName;
        juce::String fileSha256;
        juce::String detail;
        juce::String evidenceSource;
        juce::String manifestPath;
        std::uint64_t completedAtMilliseconds = 0;
        // REQUIRED: only the responder path validates a signing credential. A
        // caller that grades a result as trusted without this flag set is
        // asserting trust nothing checked.
        bool signatureChecked = false;
    };

private:
    // The worker touches nothing owned by this object. juce::Thread::stopThread
    // force-kills a worker that overruns its budget and then returns, so a
    // worker holding a raw `this` would write into freed storage; holding a
    // shared_ptr taken at run() entry keeps the state alive instead.
    struct SharedState
    {
        SharedState (const juce::String& host, int port)
            : responderHost (host), responderPortNumber (port) {}

        juce::DatagramSocket socket;
        const juce::String responderHost;
        const int responderPortNumber;
        bool socketBound = false;

        std::atomic<bool> shouldExit { false };
        juce::WaitableEvent workAvailable;
        juce::CriticalSection stateLock;
        juce::File pendingFile;
        bool hasPendingRequest = false;
        Result currentResult;

        std::atomic<std::uint64_t> responderRequests { 0 };
        std::atomic<std::uint64_t> responderAnswers { 0 };
        std::atomic<std::uint64_t> responderTimeouts { 0 };
    };

public:
    explicit VerificationClient (const juce::String& host = "127.0.0.1", int responderPort = 9877)
        : juce::Thread ("ProvenanceVerification"),
          state (std::make_shared<SharedState> (host, responderPort))
    {
        // REQUIRED: loopback only. bindToPort(0) alone binds INADDR_ANY, which
        // exposes the response socket to every interface.
        state->socketBound = state->socket.bindToPort (0, "127.0.0.1");
        startThread (juce::Thread::Priority::low);
    }

    ~VerificationClient() override
    {
        state->shouldExit.store (true, std::memory_order_release);
        signalThreadShouldExit();
        state->socket.shutdown();
        state->workAvailable.signal();
        stopThread (250);
    }

    void requestVerification (const juce::File& file)
    {
        {
            const juce::ScopedLock lock (state->stateLock);
            state->pendingFile = file;
            state->hasPendingRequest = true;
            state->currentResult = Result();
            state->currentResult.outcome = Outcome::inProgress;
            state->currentResult.fileName = file.getFileName();
            state->currentResult.detail = "Hashing the dropped file and searching local evidence.";
        }
        state->workAvailable.signal();
    }

    Result getResult() const
    {
        const juce::ScopedLock lock (state->stateLock);
        return state->currentResult;
    }

    int getResponderPort() const noexcept { return state->responderPortNumber; }
    bool isResponderSocketBound() const noexcept { return state->socketBound; }
    std::uint64_t getResponderRequests() const noexcept { return state->responderRequests.load (std::memory_order_relaxed); }
    std::uint64_t getResponderAnswers() const noexcept { return state->responderAnswers.load (std::memory_order_relaxed); }
    std::uint64_t getResponderTimeouts() const noexcept { return state->responderTimeouts.load (std::memory_order_relaxed); }

private:
    static constexpr const char* kProtocol = "apw-local-udp-verify-v1";
    static constexpr int kMaxAttempts = 2;
    static constexpr int kAttemptTimeoutMilliseconds = 250;
    static constexpr juce::int64 kMaxHashBytes = 512ll * 1024 * 1024;
    static constexpr juce::int64 kMaxManifestBytes = 4ll * 1024 * 1024;
    static constexpr int kMaxManifestsPerDirectory = 64;
    static constexpr int kMaxRiffChunks = 512;

    class CancellableInputStream final : public juce::InputStream
    {
    public:
        CancellableInputStream (juce::InputStream& sourceStream, const std::atomic<bool>& cancelFlag)
            : source (sourceStream), cancelled (cancelFlag) {}

        juce::int64 getTotalLength() override           { return source.getTotalLength(); }
        bool isExhausted() override                     { return source.isExhausted(); }
        juce::int64 getPosition() override              { return source.getPosition(); }
        bool setPosition (juce::int64 newPosition) override { return source.setPosition (newPosition); }

        int read (void* destBuffer, int maxBytesToRead) override
        {
            if (cancelled.load (std::memory_order_acquire))
                return 0;
            return source.read (destBuffer, maxBytesToRead);
        }

    private:
        juce::InputStream& source;
        const std::atomic<bool>& cancelled;
    };

    void run() override
    {
        const auto shared = state;

        while (! shared->shouldExit.load (std::memory_order_acquire))
        {
            shared->workAvailable.wait (250);
            if (shared->shouldExit.load (std::memory_order_acquire))
                return;

            juce::File file;
            bool haveWork = false;
            {
                const juce::ScopedLock lock (shared->stateLock);
                if (shared->hasPendingRequest)
                {
                    file = shared->pendingFile;
                    shared->hasPendingRequest = false;
                    haveWork = true;
                }
            }
            if (! haveWork)
                continue;

            Result result;
            if (! askResponder (*shared, file, result))
                result = verifyFromLocalEvidence (*shared, file);
            result.fileName = file.getFileName();
            result.completedAtMilliseconds = static_cast<std::uint64_t> (juce::Time::getMillisecondCounterHiRes());

            if (shared->shouldExit.load (std::memory_order_acquire))
                return;

            const juce::ScopedLock lock (shared->stateLock);
            shared->currentResult = result;
        }
    }

    static bool askResponder (SharedState& shared, const juce::File& file, Result& result)
    {
        if (! shared.socketBound)
            return false;

        const auto requestId = juce::Uuid().toDashedString();
        auto* request = new juce::DynamicObject();
        request->setProperty ("message_type", "verification_request");
        request->setProperty ("protocol", juce::String (kProtocol));
        request->setProperty ("request_id", requestId);
        request->setProperty ("file_path", file.getFullPathName());
        const auto payload = juce::JSON::toString (juce::var (request), true);
        const auto payloadBytes = static_cast<int> (payload.getNumBytesAsUTF8());

        for (int attempt = 0; attempt < kMaxAttempts
                              && ! shared.shouldExit.load (std::memory_order_acquire); ++attempt)
        {
            if (shared.socket.write (shared.responderHost, shared.responderPortNumber,
                                     payload.toRawUTF8(), payloadBytes) != payloadBytes)
                continue;
            shared.responderRequests.fetch_add (1, std::memory_order_relaxed);

            const auto deadline = juce::Time::getMillisecondCounterHiRes()
                                + static_cast<double> (kAttemptTimeoutMilliseconds);
            while (! shared.shouldExit.load (std::memory_order_acquire))
            {
                const auto remaining = static_cast<int> (deadline - juce::Time::getMillisecondCounterHiRes());
                if (remaining <= 0)
                    break;
                if (shared.socket.waitUntilReady (true, juce::jmin (remaining, 100)) <= 0)
                    continue;
                char buffer[8192] {};
                const auto bytesRead = shared.socket.read (buffer, static_cast<int> (sizeof (buffer) - 1), false);
                if (bytesRead <= 0)
                    continue;
                if (parseResponse (juce::String::fromUTF8 (buffer, bytesRead), requestId, result))
                {
                    shared.responderAnswers.fetch_add (1, std::memory_order_relaxed);
                    return true;
                }
            }
        }

        shared.responderTimeouts.fetch_add (1, std::memory_order_relaxed);
        return false;
    }

    static juce::String sanitiseUntrustedText (const juce::String& text, int maxLength)
    {
        juce::String cleaned;
        cleaned.preallocateBytes (static_cast<size_t> (maxLength));
        for (auto character = text.getCharPointer(); ! character.isEmpty(); ++character)
        {
            const auto value = *character;
            if (value < 0x20 || value == 0x7f)
                cleaned << ' ';
            else
                cleaned << juce::String::charToString (value);
            if (cleaned.length() >= maxLength)
                break;
        }
        return cleaned.trim();
    }

    static bool parseResponse (const juce::String& json, const juce::String& requestId, Result& result)
    {
        const auto parsed = safejson::parseBounded (json);
        const auto* object = parsed.getDynamicObject();
        if (object == nullptr
            || object->getProperty ("protocol").toString() != kProtocol
            || object->getProperty ("message_type").toString() != "verification_response"
            || object->getProperty ("request_id").toString() != requestId)
            return false;

        const auto outcome = object->getProperty ("outcome").toString();
        if (outcome == "verified")        result.outcome = Outcome::verified;
        else if (outcome == "changed")    result.outcome = Outcome::changed;
        else if (outcome == "untrusted")  result.outcome = Outcome::claimUntrusted;
        else if (outcome == "not_found")  result.outcome = Outcome::notFound;
        else                              return false;

        result.detail = sanitiseUntrustedText (object->getProperty ("detail").toString(), 240);
        result.fileSha256 = sanitiseUntrustedText (object->getProperty ("sha256").toString(), 64);
        result.manifestPath = sanitiseUntrustedText (object->getProperty ("manifest_path").toString(), 240);
        result.evidenceSource = "local verifier on the responder port (signatures checked there)";
        result.signatureChecked = true;
        return true;
    }

    static bool hasEmbeddedRiffClaim (const juce::File& file)
    {
        juce::FileInputStream stream (file);
        if (! stream.openedOk())
            return false;

        char header[12] {};
        if (stream.read (header, 12) != 12)
            return false;
        if (std::memcmp (header, "RIFF", 4) != 0 || std::memcmp (header + 8, "WAVE", 4) != 0)
            return false;

        const auto totalLength = stream.getTotalLength();
        for (int chunk = 0; chunk < kMaxRiffChunks; ++chunk)
        {
            char chunkId[4] {};
            if (stream.read (chunkId, 4) != 4)
                break;
            const auto chunkSize = static_cast<juce::int64> (static_cast<juce::uint32> (stream.readInt()));
            if (std::memcmp (chunkId, "C2PA", 4) == 0)
                return true;
            const auto next = stream.getPosition() + chunkSize + (chunkSize & 1);
            if (next <= stream.getPosition() || next > totalLength)
                break;
            stream.setPosition (next);
        }
        return false;
    }

    // A detached claim lives beside the audio it describes. "master.wav.c2pa"
    // and "master.c2pa" both resolve to themselves under the naive forms of
    // these two expressions, which reads a sidecar as its own evidence.
    static bool hasSidecarClaim (const juce::File& file)
    {
        const auto appended = juce::File (file.getFullPathName() + ".c2pa");
        const auto replaced = file.withFileExtension ("c2pa");
        return (appended != file && appended.existsAsFile())
            || (replaced != file && replaced.existsAsFile());
    }

    static void appendSearchDirectory (juce::Array<juce::File>& directories, const juce::File& candidate)
    {
        if (! candidate.isDirectory())
            return;
        for (const auto& existing : directories)
            if (existing == candidate)
                return;
        directories.add (candidate);
    }

    static Result verifyFromLocalEvidence (SharedState& shared, const juce::File& file)
    {
        Result result;
        result.evidenceSource = "local evidence read in the plug-in (no verifier answered; no signature was checked)";

        if (! file.existsAsFile())
        {
            result.outcome = Outcome::failed;
            result.detail = "That path is not a readable file.";
            return result;
        }

        const auto fileSize = file.getSize();
        if (fileSize <= 0 || fileSize > kMaxHashBytes)
        {
            result.outcome = Outcome::failed;
            result.detail = "File is empty or exceeds the 512 MB inspection limit; hash the file with the daemon verifier instead.";
            return result;
        }

        {
            juce::FileInputStream stream (file);
            if (! stream.openedOk())
            {
                result.outcome = Outcome::failed;
                result.detail = "The file could not be opened for hashing.";
                return result;
            }
            CancellableInputStream cancellable (stream, shared.shouldExit);
            result.fileSha256 = juce::SHA256 (cancellable).toHexString();
            if (shared.shouldExit.load (std::memory_order_acquire))
            {
                result.outcome = Outcome::failed;
                result.detail = "Inspection was cancelled while the plug-in was closing.";
                return result;
            }
        }

        juce::Array<juce::File> searchDirectories;
        const auto parent = file.getParentDirectory();
        appendSearchDirectory (searchDirectories, parent);
        appendSearchDirectory (searchDirectories, parent.getChildFile ("manifests"));
        appendSearchDirectory (searchDirectories, parent.getParentDirectory().getChildFile ("manifests"));

        enum class MatchKind { none, hashMatch, pathMismatch, nameMismatch };
        MatchKind matchKind = MatchKind::none;
        juce::File matchedManifest;
        bool matchedManifestSigned = false;

        for (const auto& directory : searchDirectories)
        {
            if (matchKind == MatchKind::hashMatch)
                break;
            int inspected = 0;
            for (const auto& entry : juce::RangedDirectoryIterator (directory, false, "*.json",
                                                                   juce::File::findFiles))
            {
                if (shared.shouldExit.load (std::memory_order_acquire))
                {
                    result.outcome = Outcome::failed;
                    result.detail = "Inspection was cancelled while the plug-in was closing.";
                    return result;
                }
                if (++inspected > kMaxManifestsPerDirectory)
                    break;
                const auto manifestFile = entry.getFile();

                const auto manifest = safejson::parseBoundedFile (manifestFile, kMaxManifestBytes);
                const auto* manifestObject = manifest.getDynamicObject();
                if (manifestObject == nullptr)
                    continue;
                const auto* exportObject = manifestObject->getProperty ("export").getDynamicObject();
                if (exportObject == nullptr)
                    continue;

                const auto recordedHash = exportObject->getProperty ("sha256").toString();
                const auto recordedPath = exportObject->getProperty ("file_path").toString();
                const auto recordedName = exportObject->getProperty ("file_name").toString();

                if (recordedHash.equalsIgnoreCase (result.fileSha256))
                {
                    matchKind = MatchKind::hashMatch;
                    matchedManifest = manifestFile;
                    matchedManifestSigned = manifestCarriesPortableSignature (*manifestObject);
                    break;
                }
                if (matchKind == MatchKind::none && recordedPath == file.getFullPathName())
                {
                    matchKind = MatchKind::pathMismatch;
                    matchedManifest = manifestFile;
                    matchedManifestSigned = manifestCarriesPortableSignature (*manifestObject);
                }
                else if (matchKind == MatchKind::none && recordedName == file.getFileName())
                {
                    matchKind = MatchKind::nameMismatch;
                    matchedManifest = manifestFile;
                    matchedManifestSigned = manifestCarriesPortableSignature (*manifestObject);
                }
            }
        }

        const auto claimPresent = hasEmbeddedRiffClaim (file) || hasSidecarClaim (file);
        const auto claimNote = claimPresent
            ? juce::String (" An embedded or sidecar C2PA claim is also present; only the verifier validates its COSE signature.")
            : juce::String();

        if (matchedManifest.existsAsFile())
            result.manifestPath = matchedManifest.getFullPathName();

        switch (matchKind)
        {
            case MatchKind::hashMatch:
                result.outcome = Outcome::boundToLocalManifest;
                result.detail = "A local manifest records this exact SHA-256, so these bytes are the ones it describes. "
                                + juce::String (matchedManifestSigned
                                    ? "It carries a portable Ed25519 signature, which this plug-in does not check."
                                    : "It carries no portable signature.")
                                + " Byte binding only: run the verifier to establish signature validity."
                                + claimNote;
                break;
            case MatchKind::pathMismatch:
                result.outcome = Outcome::changed;
                result.detail = "A manifest records this exact file path with a different SHA-256. The bytes changed after the manifest was written."
                    + claimNote;
                break;
            case MatchKind::nameMismatch:
                result.outcome = claimPresent ? Outcome::claimNotChecked : Outcome::notFound;
                result.detail = "A manifest names this filename but the bytes differ. This may be a different render rather than a modified file."
                    + claimNote;
                break;
            case MatchKind::none:
                if (claimPresent)
                {
                    result.outcome = Outcome::claimNotChecked;
                    result.detail = "An embedded or sidecar C2PA claim is present and no local manifest binds these bytes. "
                                    "This plug-in validates no signature; run the verifier to read the claim.";
                }
                else
                {
                    result.outcome = Outcome::notFound;
                    result.detail = "No embedded claim, no sidecar and no local manifest binds these bytes.";
                }
                break;
        }
        return result;
    }

    static bool manifestCarriesPortableSignature (const juce::DynamicObject& manifest)
    {
        const auto* signature = manifest.getProperty ("portable_signature").getDynamicObject();
        return signature != nullptr
            && signature->getProperty ("signature_hex").toString().isNotEmpty();
    }

    std::shared_ptr<SharedState> state;

    JUCE_DECLARE_NON_COPYABLE_WITH_LEAK_DETECTOR (VerificationClient)
};

} // namespace apw

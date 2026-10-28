<?php

declare(strict_types=1);

namespace Acme\Sdk\Http;

final class Request
{
    public function __construct(
        public readonly string $method,
        public readonly string $url,
    ) {
    }
}
